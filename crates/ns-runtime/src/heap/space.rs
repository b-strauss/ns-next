//! The object space: one reservation of `ns-memory`, split into two
//! survivor spaces, eden and the old space, in which the objects of each
//! region lie end to end from its start up to its top, each its
//! header word, its class word and its fields -- preceded by an overflow
//! word when its word count does not fit the header's byte
//! (`docs/design/memory.md`, "The object space"). A handle is the byte
//! offset of an object's header from the reservation's base. The heap
//! reads every object through `words`, the slice `[0, old top)`, so every
//! access is bounds-checked against the top and a stale offset reads a
//! wrong object at worst, never memory outside the objects.

use std::num::NonZeroU64;
use std::ops::Range;

use ns_memory::{MemoryError, Reservation};

use crate::heap::header::{Header, WORD_COUNT_OVERFLOW};

/// The byte offset of the first object's allocation: past two words that
/// no object uses, so that an all-zero word -- a missed initialization --
/// is never a live reference.
pub(in crate::heap) const FIRST_OBJECT: usize = 16;

/// The two survivor spaces and eden, HotSpot's ratio (`SurvivorRatio=8`;
/// decided with the user on 2026-10-04): the survivor spaces first, then
/// eden, then the old space, as Spur and Strongtalk lay them out
/// (`setHeapBase:memoryLimit:endOfMemory:`,
/// `prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:60020`;
/// `prior-art/strongtalk/vm/memory/universe.cpp:64-79`), so that young is
/// one comparison against `NURSERY_END`. The first survivor space starts at
/// `FIRST_OBJECT`, so it is 16 bytes smaller than the second.
pub(in crate::heap) const SURVIVOR_BYTES: usize = 1 << 20;
pub(in crate::heap) const EDEN_BYTES: usize = 8 << 20;
pub(in crate::heap) const EDEN_START: usize = 2 * SURVIVOR_BYTES;
/// Where the old space begins: a multiple of 64 words, so the forwarding
/// table's chunks never straddle the two generations.
pub(in crate::heap) const NURSERY_END: usize = EDEN_START + EDEN_BYTES;

/// What every nursery region keeps free past what it fills: the move
/// stress mode shifts a region's objects by up to `8 * (2 + 30)` bytes
/// within it, and the room for that must exist before the shift is asked.
pub(in crate::heap) const REGION_SLACK: usize = 8 * 33;

/// Whether the object at `offset` is young: one comparison, the one the
/// write barrier makes on every store.
#[inline(always)]
pub(in crate::heap) fn is_young(offset: NonZeroU64) -> bool {
    offset.get() < NURSERY_END as u64
}

/// The regions that hold objects, as a caller names them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::heap) enum RegionName {
    /// The survivor space holding the last scavenge's survivors.
    PastSurvivor,
    Eden,
    Old,
}

/// One region of the space, in byte offsets: its first possible object at
/// `base`, its objects from `start` to `top` (`start` is `base` except
/// after the move stress mode shifted them up), and its last byte before
/// `end` -- for the old space, the end of the reservation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::heap) struct Region {
    pub(in crate::heap) base: usize,
    pub(in crate::heap) start: usize,
    pub(in crate::heap) top: usize,
    pub(in crate::heap) end: usize,
}

impl Region {
    fn empty(base: usize, end: usize) -> Region {
        Region {
            base,
            start: base,
            top: base,
            end,
        }
    }

    /// Bumps the top by `words` and answers where they start; `None`, the
    /// top untouched, past `limit`.
    fn bump(&mut self, words: usize, limit: usize) -> Option<usize> {
        let end = words
            .checked_mul(8)
            .and_then(|bytes| self.top.checked_add(bytes))?;
        if end > limit {
            return None;
        }
        let start = self.top;
        self.top = end;
        Some(start)
    }
}

/// How much address space a heap reserves. Reserved space costs nothing
/// until it is committed, so this is generous; the heap aborts as running
/// out of memory once it is used up.
#[cfg(target_pointer_width = "64")]
pub(in crate::heap) const DEFAULT_RESERVATION: usize = 64 << 30;
#[cfg(not(target_pointer_width = "64"))]
pub(in crate::heap) const DEFAULT_RESERVATION: usize = 1 << 30;

/// The least a refused reservation falls back to. An address-space limit
/// (`ulimit -v`, a sandbox) can refuse `DEFAULT_RESERVATION` although a
/// heap needs far less, and a refusal there aborts every run at bootstrap;
/// below 1 GiB a program runs out of objects soon enough that aborting at
/// once is the plainer answer.
const MINIMUM_RESERVATION: usize = 1 << 30;

/// The first commit, and the least a growth commits.
const MINIMUM_COMMIT: usize = 256 << 10;

/// The top byte of an overflow word: the header's own word-count byte at
/// its overflow mark, so that a walk standing at an allocation's first word
/// tells an overflow word from a header -- a header there never carries
/// the mark, since only an object with an overflow word does
/// (Spur's `numSlotsOfAny:` and `objectAfter:`,
/// `prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:55584`, `:55781`).
const OVERFLOW_MARK: u64 = (WORD_COUNT_OVERFLOW as u64) << 56;
const OVERFLOW_COUNT: u64 = (1 << 56) - 1;

pub(in crate::heap) struct Space {
    reservation: Reservation,
    /// The two survivor spaces; `past` indexes the one holding the last
    /// scavenge's survivors, the other is empty outside a scavenge.
    survivors: [Region; 2],
    past: usize,
    eden: Region,
    old: Region,
    /// The end of the committed range, which starts at the base.
    committed: usize,
}

impl Space {
    pub(in crate::heap) fn new(reservation_bytes: usize) -> Result<Space, MemoryError> {
        let mut reservation = reserve_falling_back(reservation_bytes, Reservation::reserve)?;
        if reservation.size() < NURSERY_END + MINIMUM_COMMIT {
            return Err(MemoryError::InvalidRange);
        }
        let committed = round_up_to_page(NURSERY_END + MINIMUM_COMMIT)
            .unwrap_or(NURSERY_END + MINIMUM_COMMIT)
            .min(reservation.size());
        reservation.commit(0..committed)?;
        let old = Region::empty(NURSERY_END, reservation.size());
        Ok(Space {
            reservation,
            survivors: [
                Region::empty(FIRST_OBJECT, SURVIVOR_BYTES),
                Region::empty(SURVIVOR_BYTES, EDEN_START),
            ],
            past: 0,
            eden: Region::empty(EDEN_START, NURSERY_END),
            old,
            committed,
        })
    }

    pub(in crate::heap) fn region(&self, name: RegionName) -> Region {
        match name {
            RegionName::PastSurvivor => self.survivors[self.past],
            RegionName::Eden => self.eden,
            RegionName::Old => self.old,
        }
    }

    /// The survivor space a scavenge copies into; empty outside one.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "a later task of this road step uses it")
    )]
    pub(in crate::heap) fn future_survivor(&self) -> Region {
        self.survivors[1 - self.past]
    }

    /// The byte ranges that hold objects, in address order: every walk of
    /// the space reads these, so none can miss a region.
    pub(in crate::heap) fn occupied(&self) -> [Range<usize>; 3] {
        let past = self.survivors[self.past];
        [
            past.start..past.top,
            self.eden.start..self.eden.top,
            self.old.start..self.old.top,
        ]
    }

    pub(in crate::heap) fn occupied_bytes(&self) -> usize {
        self.occupied().iter().map(ExactSizeIterator::len).sum()
    }

    pub(in crate::heap) fn committed(&self) -> usize {
        self.committed
    }

    /// Every word below the old top: the one slice the heap reads objects
    /// through. The old top is never below `NURSERY_END`, so the slice
    /// covers the nursery.
    #[inline(always)]
    pub(in crate::heap) fn words(&self) -> &[u64] {
        match self.reservation.prefix(self.old.top) {
            Some(words) => words,
            None => unreachable!("internal: the space's top lies inside its committed prefix"),
        }
    }

    #[inline(always)]
    pub(in crate::heap) fn words_mut(&mut self) -> &mut [u64] {
        match self.reservation.prefix_mut(self.old.top) {
            Some(words) => words,
            None => unreachable!("internal: the space's top lies inside its committed prefix"),
        }
    }

    #[inline]
    pub(in crate::heap) fn allocate_young(&mut self, words: usize) -> Option<usize> {
        let limit = self.eden.end - REGION_SLACK;
        self.eden.bump(words, limit)
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "a later task of this road step uses it")
    )]
    pub(in crate::heap) fn allocate_survivor(&mut self, words: usize) -> Option<usize> {
        let future = 1 - self.past;
        let limit = self.survivors[future].end - REGION_SLACK;
        self.survivors[future].bump(words, limit)
    }

    /// The old space's bump: answers the byte offset of the first word;
    /// `None`, the top untouched, when the reservation cannot hold the
    /// words or the operating system refuses the commit. The words are
    /// whatever was there: the caller writes every one.
    #[inline]
    pub(in crate::heap) fn allocate_old(&mut self, words: usize) -> Option<usize> {
        let end = words
            .checked_mul(8)
            .and_then(|bytes| self.old.top.checked_add(bytes))?;
        if end > self.committed {
            self.ensure_committed(end)?;
        }
        self.old.bump(words, self.old.end)
    }

    /// Commits at least up to `end`: at least double what is committed and
    /// at least `MINIMUM_COMMIT`, so a run of allocations commits a
    /// logarithmic number of times; never past the reservation.
    pub(in crate::heap) fn ensure_committed(&mut self, end: usize) -> Option<()> {
        if end <= self.committed {
            return Some(());
        }
        let wanted = end
            .max(self.committed.saturating_mul(2))
            .max(MINIMUM_COMMIT);
        let target = round_up_to_page(wanted)?.min(self.reservation.size());
        if end > target {
            return None;
        }
        self.reservation.commit(self.committed..target).ok()?;
        self.committed = target;
        Some(())
    }

    /// Where a region's objects lie after a compaction or a move. Both must
    /// lie inside the region, and for the old space be committed.
    pub(in crate::heap) fn set_region_extent(
        &mut self,
        name: RegionName,
        start: usize,
        top: usize,
    ) {
        let region = match name {
            RegionName::PastSurvivor => &mut self.survivors[self.past],
            RegionName::Eden => &mut self.eden,
            RegionName::Old => &mut self.old,
        };
        debug_assert!(region.base <= start && start <= top && top <= region.end);
        debug_assert!(name != RegionName::Old || top <= self.committed);
        region.start = start;
        region.top = top;
    }

    /// Gives back the committed memory above `keep`, rounded up to a page
    /// and never below the old top or the nursery. A refused decommit
    /// leaves the range recorded as not committed (`Reservation::decommit`),
    /// so the next growth commits it again either way.
    pub(in crate::heap) fn release_above(&mut self, keep: usize) {
        let Some(keep) = round_up_to_page(keep.max(self.old.top).max(NURSERY_END + MINIMUM_COMMIT))
        else {
            return;
        };
        if keep < self.committed {
            let _ = self.reservation.decommit(keep..self.committed);
            self.committed = keep;
        }
    }
}

/// Reserves `requested` bytes through `reserve`; when the operating system
/// refuses, halves the request and tries again, never below
/// `MINIMUM_RESERVATION` (a request already below it is tried alone). The
/// last refusal is the answer. Only a refusal is retried: any other error
/// would recur at every size.
fn reserve_falling_back<T>(
    requested: usize,
    mut reserve: impl FnMut(usize) -> Result<T, MemoryError>,
) -> Result<T, MemoryError> {
    let mut bytes = requested;
    loop {
        match reserve(bytes) {
            Err(MemoryError::Refused { .. }) if bytes > MINIMUM_RESERVATION => {
                bytes = (bytes / 2).max(MINIMUM_RESERVATION);
            }
            answer => return answer,
        }
    }
}

fn round_up_to_page(bytes: usize) -> Option<usize> {
    let page = Reservation::page_size();
    Some(bytes.checked_add(page - 1)? / page * page)
}

/// The words an object of `word_count` words after its header takes: its
/// header and those words, and an overflow word when the header's byte
/// cannot hold the count.
/// Saturating, so that a count an image's first pass read cannot overflow
/// before the allocation refuses it.
pub(in crate::heap) fn allocation_words(word_count: usize) -> usize {
    word_count.saturating_add(1 + usize::from(word_count >= WORD_COUNT_OVERFLOW))
}

/// The overflow word of an object of `word_count` words after its header.
pub(in crate::heap) fn overflow_word(word_count: usize) -> u64 {
    OVERFLOW_MARK | (word_count as u64 & OVERFLOW_COUNT)
}

/// The object whose allocation begins at word `position`: its header's
/// word index, and the word index at which the next allocation begins.
pub(in crate::heap) fn object_at(words: &[u64], position: usize) -> (usize, usize) {
    let first = words[position];
    if first & OVERFLOW_MARK == OVERFLOW_MARK {
        let header = position + 1;
        (header, header + 1 + (first & OVERFLOW_COUNT) as usize)
    } else {
        (position, position + 1 + Header(first).word_count())
    }
}

/// The words of the object whose header is word `header`: the header and
/// every word after it, the count read from the overflow word when the
/// header carries the mark.
#[inline(always)]
pub(in crate::heap) fn block_range(words: &[u64], header: usize) -> std::ops::Range<usize> {
    let count = match Header(words[header]).word_count() {
        WORD_COUNT_OVERFLOW => (words[header - 1] & OVERFLOW_COUNT) as usize,
        count => count,
    };
    header..header + 1 + count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::header::{Format, Header};
    use crate::mixin::DispatchId;

    const SMALL: usize = 16 << 20;

    /// Writes an object of `word_count` words after its header at the
    /// allocation `start` (a byte offset), its overflow word first when it
    /// needs one, and answers its header's word index.
    fn write_object(space: &mut Space, start: usize, word_count: usize) -> usize {
        let words = space.words_mut();
        let mut header = start / 8;
        if word_count >= WORD_COUNT_OVERFLOW {
            words[header] = overflow_word(word_count);
            header += 1;
        }
        words[header] = Header::new(DispatchId::NONE, Format::FixedPointers, 0, word_count).0;
        header
    }

    #[test]
    fn a_new_space_is_four_empty_regions_with_the_nursery_committed() {
        let space = Space::new(SMALL).expect("reserve");
        let past = space.region(RegionName::PastSurvivor);
        let future = space.future_survivor();
        let eden = space.region(RegionName::Eden);
        let old = space.region(RegionName::Old);
        assert_eq!((past.base, past.end), (FIRST_OBJECT, SURVIVOR_BYTES));
        assert_eq!((future.base, future.end), (SURVIVOR_BYTES, EDEN_START));
        assert_eq!((eden.base, eden.end), (EDEN_START, NURSERY_END));
        assert_eq!(
            (old.base, old.start, old.top),
            (NURSERY_END, NURSERY_END, NURSERY_END)
        );
        for region in [past, future, eden] {
            assert_eq!((region.start, region.top), (region.base, region.base));
        }
        assert!(space.committed() >= NURSERY_END + MINIMUM_COMMIT);
        assert_eq!(space.occupied_bytes(), 0);
        assert_eq!(space.words().len(), NURSERY_END / 8);
    }

    #[test]
    fn each_region_bumps_its_own_top() {
        let mut space = Space::new(SMALL).expect("reserve");
        assert_eq!(space.allocate_old(2), Some(NURSERY_END));
        assert_eq!(space.allocate_old(5), Some(NURSERY_END + 16));
        assert_eq!(space.allocate_young(3), Some(EDEN_START));
        assert_eq!(space.allocate_survivor(4), Some(SURVIVOR_BYTES));
        assert_eq!(space.region(RegionName::Old).top, NURSERY_END + 56);
        assert_eq!(space.region(RegionName::Eden).top, EDEN_START + 24);
        assert_eq!(
            space.occupied_bytes(),
            56 + 24,
            "the future survivor space is not occupied yet"
        );
        let [past, eden, old] = space.occupied();
        assert_eq!(past, FIRST_OBJECT..FIRST_OBJECT);
        assert_eq!(eden, EDEN_START..EDEN_START + 24);
        assert_eq!(old, NURSERY_END..NURSERY_END + 56);
    }

    #[test]
    fn eden_and_a_survivor_space_refuse_what_would_cross_their_slack() {
        let mut space = Space::new(SMALL).expect("reserve");
        let eden_words = (EDEN_BYTES - REGION_SLACK) / 8;
        assert_eq!(space.allocate_young(eden_words + 1), None);
        assert_eq!(space.allocate_young(eden_words), Some(EDEN_START));
        assert_eq!(space.allocate_young(1), None, "eden is spent");
        let survivor_words = (SURVIVOR_BYTES - REGION_SLACK) / 8;
        assert_eq!(space.allocate_survivor(survivor_words + 1), None);
        assert!(space.allocate_survivor(survivor_words).is_some());
    }

    #[test]
    fn young_means_below_the_nursery_end() {
        let offset = |bytes: u64| NonZeroU64::new(bytes).expect("non-zero");
        assert!(is_young(offset(FIRST_OBJECT as u64)));
        assert!(is_young(offset(NURSERY_END as u64 - 8)));
        assert!(!is_young(offset(NURSERY_END as u64)));
    }

    #[test]
    fn one_allocation_larger_than_everything_committed_commits_enough_at_once() {
        // Three times what the nursery's commit holds no longer fits `SMALL`.
        let mut space = Space::new(4 * SMALL).expect("reserve");
        let committed = space.committed();
        let words = committed / 8 * 3;
        let start = space.allocate_old(words).expect("inside the reservation");
        assert!(space.committed() >= start + words * 8);
        space.words_mut()[start / 8 + words - 1] = 7;
        assert_eq!(space.words()[start / 8 + words - 1], 7);
    }

    #[test]
    fn an_allocation_past_the_reservation_answers_none_and_leaves_the_top() {
        let mut space = Space::new(SMALL).expect("reserve");
        let top = space.region(RegionName::Old).top;
        assert_eq!(
            space.allocate_old(usize::MAX / 8),
            None,
            "an overflowing size"
        );
        assert_eq!(
            space.allocate_old(SMALL),
            None,
            "eight times the reservation"
        );
        assert_eq!(space.region(RegionName::Old).top, top);
    }

    #[test]
    fn the_walk_finds_every_object_an_overflow_word_included() {
        let mut space = Space::new(SMALL).expect("reserve");
        // 254 is the widest count the header's byte holds, 255 the first
        // in an overflow word and the overflow mark itself.
        let sizes = [1, 300, 3, 254, 255, 256];
        let mut headers = Vec::new();
        for size in sizes {
            let start = space.allocate_old(allocation_words(size)).expect("room");
            headers.push(write_object(&mut space, start, size));
        }
        let words = space.words();
        let old = space.region(RegionName::Old);
        let mut position = old.start / 8;
        let mut found = Vec::new();
        while position < old.top / 8 {
            let (header, next) = object_at(words, position);
            assert_eq!(block_range(words, header).end, next);
            found.push(header);
            position = next;
        }
        assert_eq!(found, headers);
        assert_eq!(
            headers[1],
            headers[0] + 2 + 1,
            "the large object's header follows its overflow word"
        );
        assert_eq!(block_range(words, headers[1]).len(), 1 + 300);
        assert_eq!(
            headers[4],
            headers[3] + 1 + 254 + 1,
            "254 takes no overflow word"
        );
        assert_eq!(headers[5], headers[4] + 1 + 255 + 1, "255 takes one");
        for (header, size) in headers.iter().zip(sizes) {
            assert_eq!(block_range(words, *header).len(), 1 + size);
        }
    }

    const REFUSED: MemoryError = MemoryError::Refused {
        operation: ns_memory::Operation::Reserve,
        code: 0,
    };

    /// Runs the fallback against a reserve that refuses every size above
    /// `largest_granted`, and answers the sizes tried and the outcome.
    fn reserve_granting_up_to(
        requested: usize,
        largest_granted: usize,
    ) -> (Vec<usize>, Result<usize, MemoryError>) {
        let mut tried = Vec::new();
        let answer = reserve_falling_back(requested, |bytes| {
            tried.push(bytes);
            if bytes <= largest_granted {
                Ok(bytes)
            } else {
                Err(REFUSED)
            }
        });
        (tried, answer)
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn a_refused_reservation_halves_down_to_the_floor_and_then_answers_the_refusal() {
        const GIB: usize = 1 << 30;
        let (tried, answer) = reserve_granting_up_to(64 * GIB, 0);
        assert_eq!(
            tried,
            [64, 32, 16, 8, 4, 2, 1].map(|gibibytes| gibibytes * GIB)
        );
        assert!(matches!(answer, Err(MemoryError::Refused { .. })));

        let (tried, answer) = reserve_granting_up_to(64 * GIB, 8 * GIB);
        assert_eq!(tried, [64, 32, 16, 8].map(|gibibytes| gibibytes * GIB));
        assert!(matches!(answer, Ok(bytes) if bytes == 8 * GIB));

        let (tried, _) = reserve_granting_up_to(3 * GIB, 0);
        assert_eq!(tried, [3 * GIB, 3 * GIB / 2, GIB], "never below the floor");
    }

    #[test]
    fn a_request_below_the_floor_is_tried_alone_and_other_errors_are_not_retried() {
        let (tried, answer) = reserve_granting_up_to(SMALL, 0);
        assert_eq!(tried, [SMALL]);
        assert!(matches!(answer, Err(MemoryError::Refused { .. })));

        let mut tried = Vec::new();
        let answer: Result<usize, MemoryError> =
            reserve_falling_back(MINIMUM_RESERVATION * 2, |bytes| {
                tried.push(bytes);
                Err(MemoryError::InvalidRange)
            });
        assert_eq!(tried, [MINIMUM_RESERVATION * 2]);
        assert!(matches!(answer, Err(MemoryError::InvalidRange)));
    }

    #[test]
    fn releasing_never_cuts_below_the_top_and_gives_back_what_lies_above() {
        let mut space = Space::new(SMALL).expect("reserve");
        space
            .allocate_old(SMALL / 8 / 4)
            .expect("a quarter of the reservation");
        let grown = space.committed();
        space.set_region_extent(RegionName::Old, NURSERY_END, NURSERY_END + 64);
        space.release_above(0);
        assert!(space.committed() < grown);
        assert!(space.committed() >= NURSERY_END + MINIMUM_COMMIT);
        assert_eq!(space.words().len(), (NURSERY_END + 64) / 8);
        assert!(
            space.allocate_old(2).is_some(),
            "it grows again after a release"
        );
    }
}
