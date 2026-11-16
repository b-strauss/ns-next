//! The relocations a collection and a move forward every handle through
//! (`Relocation`, `heap.rs`).

use std::num::NonZeroU64;
use std::ops::Range;

use crate::heap::Relocation;

/// Where a collection slides each survivor: one bit per word of the space
/// for every word of a surviving object, its overflow word included, and
/// per chunk of 64 words the live words before the chunk. A survivor's new
/// position is the first object's plus the live words before it -- one
/// table entry, a mask and a `popcount` per handle, in about 3% of the
/// space (decided with the user on 2026-10-03, against a binary search of
/// a sorted list and a dense table half the size of the space;
/// `docs/design/memory.md`, "Compaction and forwarding").
///
/// The table is indexed from word 0 through the old space's top, so a
/// lookup is one index and no range check, but a collection writes and
/// reads only the chunks that cover the occupied ranges -- the past
/// survivor space's and eden's filled prefixes and the old space -- and
/// the buffer outlives the collection (`IdleTable`): a full collection
/// over a few live objects costs what they occupy, not the nursery's
/// whole reservation, which the table spans (`memory.md`, "Compaction and
/// forwarding").
///
/// Every chunk outside `covered` is clear -- no live bit, no prefix --
/// whenever a table exists: the buffer comes clear (`IdleTable`), and only
/// `set_live` and `summarize` write it, both inside `covered`. So a word in
/// a gap never reads as live, and `is_live` needs no look at the ranges.
pub(crate) struct ForwardingTable {
    chunks: Vec<Chunk>,
    /// The chunks covering the occupied word ranges, in the order
    /// `summarize` counts them: the old space's, the past survivor space's,
    /// then eden's.
    covered: [Range<usize>; 3],
    /// The word index the first survivor moves to.
    base: usize,
}

/// A forwarding table's buffer between two collections, every chunk clear.
/// The heap keeps it, so that a collection neither allocates nor zeroes a
/// table spanning the whole nursery. A collection's table may cover other
/// chunks than the last one's -- eden's prefix was longer, or the old space
/// higher -- so the bits a table set are cleared as it is given back
/// (`ForwardingTable::into_idle`), which keeps every chunk clear without
/// touching the gaps; a table dropped instead takes its buffer with it, and
/// the next collection starts from an empty one.
#[derive(Default)]
pub(crate) struct IdleTable {
    chunks: Vec<Chunk>,
}

/// The bits and the prefix side by side, so one lookup reads one cache line.
#[derive(Clone, Copy, Default)]
struct Chunk {
    live: u64,
    before: u64,
}

/// The chunks that hold the words of `words`; none for an empty range.
fn chunks_of(words: &Range<usize>) -> Range<usize> {
    if words.is_empty() {
        0..0
    } else {
        words.start / 64..words.end.div_ceil(64)
    }
}

impl ForwardingTable {
    /// The table for a collection whose occupied word ranges are `past`,
    /// `eden` and `old`, its first survivor moving to word `base_word`,
    /// over `idle`'s buffer. A buffer short of the highest of them grows to
    /// the chunks of the first `committed_words` words of the space, not
    /// just to the old top: committed memory grows by doubling
    /// (`Space::ensure_committed`), so an old top that rises at every
    /// collection reallocates and copies the buffer only as often as the
    /// space commits more, and `into_idle` keeps what it grew.
    ///
    /// No two ranges share a chunk, which `summarize` would count twice:
    /// the regions' reservations meet at multiples of a chunk's 512 bytes
    /// (`space.rs`), as a debug build checks.
    pub(in crate::heap) fn over(
        idle: IdleTable,
        [past, eden, old]: [Range<usize>; 3],
        base_word: usize,
        committed_words: usize,
    ) -> ForwardingTable {
        let covered = [chunks_of(&old), chunks_of(&past), chunks_of(&eden)];
        if cfg!(debug_assertions) {
            let mut occupied: Vec<&Range<usize>> =
                covered.iter().filter(|range| !range.is_empty()).collect();
            occupied.sort_by_key(|range| range.start);
            assert!(
                occupied.windows(2).all(|pair| pair[0].end <= pair[1].start),
                "internal: two occupied regions share a forwarding table chunk"
            );
        }
        let needed = covered.iter().map(|range| range.end).max().unwrap_or(0);
        let mut chunks = idle.chunks;
        if chunks.len() < needed {
            let length = needed.max(committed_words.div_ceil(64));
            chunks.reserve_exact(length - chunks.len());
            chunks.resize(length, Chunk::default());
        }
        ForwardingTable {
            chunks,
            covered,
            base: base_word,
        }
    }

    /// Marks the words of one surviving object's allocation as live; they
    /// lie inside one of the occupied ranges the table was made `over`,
    /// or the invariant on the gaps would break.
    pub(in crate::heap) fn set_live(&mut self, words: Range<usize>) {
        debug_assert!(
            self.covered.iter().any(|range| {
                range.start <= words.start / 64 && words.end.div_ceil(64) <= range.end
            }),
            "internal: a survivor outside the occupied ranges"
        );
        let mut word = words.start;
        while word < words.end {
            let bit = word % 64;
            let span = (64 - bit).min(words.end - word);
            let mask = if span == 64 {
                u64::MAX
            } else {
                ((1u64 << span) - 1) << bit
            };
            self.chunks[word / 64].live |= mask;
            word += span;
        }
    }

    /// Fills in each covered chunk's prefix: the old space's first, then
    /// the nursery's in address order -- the past survivor space's, then
    /// eden's -- so that the old space's survivors keep its bottom and the
    /// nursery's follow them, as Strongtalk's full collection compacts the
    /// new generation into the old one after the old one's own survivors
    /// (`prior-art/strongtalk/vm/memory/markSweep.cpp:255`). A chunk in a
    /// gap holds no live word, so it adds nothing to a prefix, and no
    /// destination is asked of it. Answers the live words in all.
    pub(in crate::heap) fn summarize(&mut self) -> usize {
        let mut total = 0u64;
        for range in self.covered.clone() {
            for chunk in &mut self.chunks[range] {
                chunk.before = total;
                total += u64::from(chunk.live.count_ones());
            }
        }
        usize::try_from(total).expect(
            "internal: the live words lie inside the object space's reservation, DEFAULT_RESERVATION",
        )
    }

    fn is_live(&self, word: usize) -> bool {
        self.chunks
            .get(word / 64)
            .is_some_and(|chunk| chunk.live >> (word % 64) & 1 == 1)
    }

    /// The new word index of live word `word`.
    pub(in crate::heap) fn destination(&self, word: usize) -> usize {
        let chunk = self.chunks[word / 64];
        let below = chunk.live & ((1u64 << (word % 64)) - 1);
        self.base
            + usize::try_from(chunk.before + u64::from(below.count_ones())).expect(
                "internal: a word index lies inside the object space's reservation, DEFAULT_RESERVATION",
            )
    }

    /// The buffer, every chunk clear again -- only the covered ones can
    /// hold a bit or a prefix, so only they are cleared -- and kept to the
    /// chunks of the first `committed_words` words of the space: its
    /// footprint, about 3% of them, follows the committed memory, which a
    /// collection gives back past its hysteresis (`Space::release_above`),
    /// not the highest top the space ever had. The next collection's old
    /// top lies inside the committed memory unless the space grew, so it
    /// grows the buffer only then.
    pub(in crate::heap) fn into_idle(self, committed_words: usize) -> IdleTable {
        let mut chunks = self.chunks;
        for range in self.covered {
            chunks[range].fill(Chunk::default());
        }
        let keep = committed_words.div_ceil(64);
        if chunks.capacity() > keep {
            chunks.truncate(keep);
            chunks.shrink_to(keep);
        }
        IdleTable { chunks }
    }
}

impl Relocation for ForwardingTable {
    fn apply(&self, offset: NonZeroU64) -> NonZeroU64 {
        let word = usize::try_from(offset.get() / 8).expect(
            "internal: a word index lies inside the object space's reservation, DEFAULT_RESERVATION",
        );
        debug_assert!(
            self.is_live(word),
            "a handle to a collected object survived marking: a root the tracer does not know about"
        );
        match NonZeroU64::new(8 * self.destination(word) as u64) {
            Some(destination) => destination,
            None => unreachable!("internal: a destination lies past the first object"),
        }
    }
}

/// The move stress mode's relocation: each region's objects shift within
/// that region by their own distance, so every offset changes, young stays
/// young, and a handle the tracer missed both carries an old epoch and
/// names a different place.
#[cfg(debug_assertions)]
pub(crate) struct Shift {
    pub(in crate::heap) regions: [ShiftedRegion; 3],
}

#[cfg(debug_assertions)]
#[derive(Clone)]
pub(crate) struct ShiftedRegion {
    pub(in crate::heap) source: Range<u64>,
    pub(in crate::heap) destination: u64,
}

#[cfg(debug_assertions)]
impl Relocation for Shift {
    fn apply(&self, offset: NonZeroU64) -> NonZeroU64 {
        for region in &self.regions {
            if region.source.contains(&offset.get())
                && let Some(moved) =
                    NonZeroU64::new(offset.get() - region.source.start + region.destination)
            {
                return moved;
            }
        }
        unreachable!("internal: a moved handle names an object in no occupied region")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::space::FIRST_OBJECT;

    /// A table over a fresh buffer whose only occupied range is `old`.
    fn over_old(old: Range<usize>, base_word: usize) -> ForwardingTable {
        ForwardingTable::over(IdleTable::default(), [0..0, 0..0, old], base_word, 0)
    }

    #[test]
    fn set_live_across_a_chunk_boundary_sets_exactly_those_bits() {
        let mut table = over_old(0..192, FIRST_OBJECT / 8);
        table.set_live(60..70);
        assert_eq!(table.chunks[0].live, 0b1111 << 60);
        assert_eq!(table.chunks[1].live, 0b11_1111);
        assert_eq!(table.chunks[2].live, 0);
        table.set_live(128..192);
        assert_eq!(table.chunks[2].live, u64::MAX, "a whole chunk at once");
        assert_eq!(table.summarize(), 10 + 64);
    }

    #[test]
    fn the_old_segment_keeps_the_bottom_and_the_nursery_follows_it() {
        // Base 128 words, the old segment from chunk 2 on; one young object
        // at words 10..13 and two old ones at 130..132 and 200..204.
        let mut table =
            ForwardingTable::over(IdleTable::default(), [0..0, 0..64, 128..256], 128, 0);
        table.set_live(10..13);
        table.set_live(130..132);
        table.set_live(200..204);
        assert_eq!(table.summarize(), 3 + 2 + 4);
        assert_eq!(table.destination(130), 128);
        assert_eq!(table.destination(200), 128 + 2);
        assert_eq!(table.destination(10), 128 + 6, "after every old survivor");
        assert_eq!(table.destination(12), 128 + 8);
    }

    #[test]
    fn the_first_live_word_goes_to_the_first_object() {
        let mut table = over_old(0..256, FIRST_OBJECT / 8);
        table.set_live(100..104);
        table.summarize();
        assert_eq!(table.destination(100), FIRST_OBJECT / 8);
    }

    #[test]
    fn a_word_after_a_dead_gap_goes_past_the_live_words_before_it() {
        let mut table = over_old(0..256, FIRST_OBJECT / 8);
        table.set_live(2..5);
        table.set_live(62..66);
        table.set_live(200..203);
        table.summarize();
        assert_eq!(table.destination(62), FIRST_OBJECT / 8 + 3);
        assert_eq!(table.destination(65), FIRST_OBJECT / 8 + 3 + 3);
        assert_eq!(table.destination(200), FIRST_OBJECT / 8 + 3 + 4);
        assert!(!table.is_live(5));
        assert!(table.is_live(202));
    }

    /// The past survivor space, eden and the old space lie apart, with
    /// unoccupied chunks between them: `summarize` counts the three in
    /// order and writes no chunk of a gap, which reads as dead.
    #[test]
    fn the_chunks_between_the_occupied_ranges_are_neither_counted_nor_written() {
        // Past at words 64..100, eden at 256..300, old from 640 on: chunks
        // 2..4 and 5..10 are gaps.
        let mut table =
            ForwardingTable::over(IdleTable::default(), [64..100, 256..300, 640..700], 640, 0);
        table.set_live(70..72);
        table.set_live(260..263);
        table.set_live(650..654);
        assert_eq!(table.summarize(), 2 + 3 + 4);
        assert_eq!(table.destination(650), 640);
        assert_eq!(
            table.destination(70),
            640 + 4,
            "the past survivors follow the old ones"
        );
        assert_eq!(
            table.destination(260),
            640 + 4 + 2,
            "and eden's follow them"
        );
        for gap in [2, 3, 5, 6, 7, 8, 9] {
            assert_eq!(table.chunks[gap].live, 0);
            assert_eq!(
                table.chunks[gap].before, 0,
                "summarize never wrote gap chunk {gap}"
            );
        }
        assert!(!table.is_live(200));
        assert!(!table.is_live(400));
    }

    /// A collection whose eden is shorter than the last one's, over the
    /// buffer the last one gave back, answers what a fresh table answers:
    /// the bits the earlier collection set -- inside a chunk the later one
    /// covers too, and past its eden's end -- are gone.
    #[test]
    fn a_shorter_eden_after_a_longer_one_reads_no_stale_bit() {
        let mut first =
            ForwardingTable::over(IdleTable::default(), [0..0, 64..640, 1024..1100], 1024, 0);
        first.set_live(80..90);
        first.set_live(500..510);
        first.set_live(1024..1030);
        first.set_live(1060..1062);
        assert_eq!(first.summarize(), 10 + 10 + 6 + 2);
        let idle = first.into_idle(2048);

        let later = |idle: IdleTable| {
            let mut table = ForwardingTable::over(idle, [0..0, 64..128, 1024..1100], 1024, 0);
            table.set_live(70..72);
            table.set_live(1024..1030);
            table.set_live(1070..1072);
            let live = table.summarize();
            (live, table)
        };
        let (recycled_live, recycled) = later(idle);
        let (fresh_live, fresh) = later(IdleTable::default());
        assert_eq!(recycled_live, fresh_live);
        assert_eq!(recycled_live, 2 + 6 + 2);
        for word in [70, 71, 1024, 1029, 1070, 1071] {
            assert_eq!(recycled.destination(word), fresh.destination(word));
        }
        assert!(!recycled.is_live(85), "a stale bit inside a covered chunk");
        assert!(!recycled.is_live(505), "a stale bit past the shorter eden");
        assert!(!recycled.is_live(1060), "a stale bit in the old space");
    }

    /// A buffer that grows grows to the committed words, so a later
    /// collection whose old top rose inside them finds it long enough and
    /// copies nothing.
    #[test]
    fn a_buffer_grows_to_the_committed_words() {
        let first = ForwardingTable::over(IdleTable::default(), [0..0, 0..0, 0..640], 0, 64 * 100);
        assert_eq!(
            first.chunks.len(),
            100,
            "the committed words' chunks, not the top's"
        );
        let idle = first.into_idle(64 * 100);
        let buffer = idle.chunks.as_ptr();
        let second = ForwardingTable::over(idle, [0..0, 0..0, 0..64 * 50], 0, 64 * 100);
        assert_eq!(
            second.chunks.as_ptr(),
            buffer,
            "the old top rose inside the buffer"
        );
        assert_eq!(second.chunks.len(), 100);
    }

    /// The buffer given back keeps only the chunks of the committed words,
    /// so a heap that shrank does not keep the table of its largest top.
    #[test]
    fn the_idle_buffer_follows_the_committed_memory() {
        let mut table = over_old(0..64 * 100, FIRST_OBJECT / 8);
        table.set_live(64 * 99..64 * 99 + 3);
        table.summarize();
        let idle = table.into_idle(64 * 10);
        assert_eq!(idle.chunks.len(), 10);
        assert!(
            idle.chunks.capacity() < 100,
            "the memory past the committed words is freed"
        );
        assert!(
            idle.chunks
                .iter()
                .all(|chunk| chunk.live == 0 && chunk.before == 0)
        );
    }

    /// A survivor outside the occupied ranges would set a bit in a gap,
    /// which nothing clears.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "a survivor outside the occupied ranges")]
    fn a_survivor_in_a_gap_is_refused() {
        let mut table =
            ForwardingTable::over(IdleTable::default(), [0..0, 64..128, 640..700], 640, 0);
        table.set_live(200..202);
    }
}
