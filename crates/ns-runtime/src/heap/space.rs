//! The object space: one reservation of `ns-memory`, in which every object
//! of a heap lies end to end from `FIRST_OBJECT` up to the top, each its
//! header word, its class word and its fields -- preceded by an overflow
//! word when its word count does not fit the header's byte
//! (`docs/design/memory.md`, "The object space"). A handle is the byte
//! offset of an object's header from the reservation's base. The heap
//! reads every object through `words`, the slice `[0, top)`, so every
//! access is bounds-checked against the top and a stale offset reads a
//! wrong object at worst, never memory outside the objects.

use ns_memory::{MemoryError, Reservation};

use crate::heap::header::{Header, WORD_COUNT_OVERFLOW};

/// The byte offset of the first object's allocation: past two words that
/// no object uses, so that an all-zero word -- a missed initialization --
/// is never a live reference.
pub(in crate::heap) const FIRST_OBJECT: usize = 16;

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
    /// Where the first object's allocation starts: `FIRST_OBJECT`, except
    /// after the move stress mode shifted every object up.
    start: usize,
    /// Where the next allocation starts; every byte below it from `start`
    /// is an object.
    top: usize,
    /// The end of the committed range, which starts at the base.
    committed: usize,
}

impl Space {
    pub(in crate::heap) fn new(reservation_bytes: usize) -> Result<Space, MemoryError> {
        let mut reservation = reserve_falling_back(reservation_bytes, Reservation::reserve)?;
        let committed = round_up_to_page(MINIMUM_COMMIT)
            .unwrap_or(MINIMUM_COMMIT)
            .min(reservation.size());
        reservation.commit(0..committed)?;
        Ok(Space {
            reservation,
            start: FIRST_OBJECT,
            top: FIRST_OBJECT,
            committed,
        })
    }

    pub(in crate::heap) fn start(&self) -> usize {
        self.start
    }

    pub(in crate::heap) fn top(&self) -> usize {
        self.top
    }

    pub(in crate::heap) fn committed(&self) -> usize {
        self.committed
    }

    /// Every word below the top: the one slice the heap reads objects
    /// through.
    #[inline(always)]
    pub(in crate::heap) fn words(&self) -> &[u64] {
        match self.reservation.prefix(self.top) {
            Some(words) => words,
            None => unreachable!("internal: the space's top lies inside its committed prefix"),
        }
    }

    #[inline(always)]
    pub(in crate::heap) fn words_mut(&mut self) -> &mut [u64] {
        match self.reservation.prefix_mut(self.top) {
            Some(words) => words,
            None => unreachable!("internal: the space's top lies inside its committed prefix"),
        }
    }

    /// Bumps the top by `words` and answers the byte offset of the first;
    /// `None`, the top untouched, when the reservation cannot hold them or
    /// the operating system refuses the commit. The words are whatever was
    /// there: the caller writes every one.
    #[inline]
    pub(in crate::heap) fn allocate(&mut self, words: usize) -> Option<usize> {
        let end = words
            .checked_mul(8)
            .and_then(|bytes| self.top.checked_add(bytes))?;
        if end > self.committed {
            self.ensure_committed(end)?;
        }
        let start = self.top;
        self.top = end;
        Some(start)
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

    /// Where the objects lie after a compaction or a move. Both must be
    /// committed.
    pub(in crate::heap) fn set_extent(&mut self, start: usize, top: usize) {
        debug_assert!(FIRST_OBJECT <= start && start <= top && top <= self.committed);
        self.start = start;
        self.top = top;
    }

    /// Gives back the committed memory above `keep`, rounded up to a page
    /// and never below the top. A refused decommit leaves the range
    /// recorded as not committed (`Reservation::decommit`), so the next
    /// growth commits it again either way.
    pub(in crate::heap) fn release_above(&mut self, keep: usize) {
        let Some(keep) = round_up_to_page(keep.max(self.top).max(MINIMUM_COMMIT)) else {
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
    fn a_new_space_is_empty_and_starts_at_the_first_object() {
        let space = Space::new(SMALL).expect("reserve");
        assert_eq!((space.start(), space.top()), (FIRST_OBJECT, FIRST_OBJECT));
        assert_eq!(space.words().len(), FIRST_OBJECT / 8);
    }

    #[test]
    fn allocation_bumps_the_top_by_whole_words() {
        let mut space = Space::new(SMALL).expect("reserve");
        assert_eq!(space.allocate(2), Some(FIRST_OBJECT));
        assert_eq!(space.allocate(5), Some(FIRST_OBJECT + 16));
        assert_eq!(space.top(), FIRST_OBJECT + 56);
        assert_eq!(space.words().len(), (FIRST_OBJECT + 56) / 8);
    }

    #[test]
    fn one_allocation_larger_than_everything_committed_commits_enough_at_once() {
        let mut space = Space::new(SMALL).expect("reserve");
        let committed = space.committed();
        let words = committed / 8 * 3;
        let start = space.allocate(words).expect("inside the reservation");
        assert!(space.committed() >= start + words * 8);
        space.words_mut()[start / 8 + words - 1] = 7;
        assert_eq!(space.words()[start / 8 + words - 1], 7);
    }

    #[test]
    fn an_allocation_past_the_reservation_answers_none_and_leaves_the_top() {
        let mut space = Space::new(SMALL).expect("reserve");
        let top = space.top();
        assert_eq!(space.allocate(usize::MAX / 8), None, "an overflowing size");
        assert_eq!(space.allocate(SMALL), None, "eight times the reservation");
        assert_eq!(space.top(), top);
    }

    #[test]
    fn the_walk_finds_every_object_an_overflow_word_included() {
        let mut space = Space::new(SMALL).expect("reserve");
        // 254 is the widest count the header's byte holds, 255 the first
        // in an overflow word and the overflow mark itself.
        let sizes = [1, 300, 3, 254, 255, 256];
        let mut headers = Vec::new();
        for size in sizes {
            let start = space.allocate(allocation_words(size)).expect("room");
            headers.push(write_object(&mut space, start, size));
        }
        let words = space.words();
        let mut position = space.start() / 8;
        let mut found = Vec::new();
        while position < space.top() / 8 {
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
            .allocate(SMALL / 8 / 4)
            .expect("a quarter of the reservation");
        let grown = space.committed();
        space.set_extent(FIRST_OBJECT, FIRST_OBJECT + 64);
        space.release_above(0);
        assert!(space.committed() < grown);
        assert!(space.committed() >= space.top());
        assert_eq!(space.words().len(), (FIRST_OBJECT + 64) / 8);
        assert!(
            space.allocate(2).is_some(),
            "it grows again after a release"
        );
    }
}
