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
/// space for as long as the collection runs (decided with the user on
/// 2026-10-03, against a binary search of a sorted list and a dense table
/// half the size of the space; `docs/design/memory.md`, "Compaction and
/// forwarding").
pub(crate) struct ForwardingTable {
    chunks: Vec<Chunk>,
    /// The word index the first survivor moves to.
    base: usize,
}

/// The bits and the prefix side by side, so one lookup reads one cache line.
#[derive(Clone, Copy, Default)]
struct Chunk {
    live: u64,
    before: u64,
}

impl ForwardingTable {
    pub(in crate::heap) fn with_words(words: usize, base_word: usize) -> ForwardingTable {
        ForwardingTable {
            chunks: vec![Chunk::default(); words.div_ceil(64)],
            base: base_word,
        }
    }

    /// Marks the words of one surviving object's allocation as live.
    pub(in crate::heap) fn set_live(&mut self, words: Range<usize>) {
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

    /// Fills in each chunk's prefix: the chunks from `first_chunk` on -- the
    /// old space -- first, then the ones below it -- the nursery -- so that
    /// the old space's survivors keep its bottom and the nursery's follow
    /// them, as Strongtalk's full collection compacts the new generation into
    /// the old one after the old one's own survivors
    /// (`prior-art/strongtalk/vm/memory/markSweep.cpp:255`). Answers the live
    /// words in all.
    pub(in crate::heap) fn summarize(&mut self, first_chunk: usize) -> usize {
        let first_chunk = first_chunk.min(self.chunks.len());
        let (nursery, old) = self.chunks.split_at_mut(first_chunk);
        let mut total = 0u64;
        for chunk in old.iter_mut().chain(nursery.iter_mut()) {
            chunk.before = total;
            total += u64::from(chunk.live.count_ones());
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
}

impl Relocation for ForwardingTable {
    fn apply(&self, offset: NonZeroU64) -> NonZeroU64 {
        let word = (offset.get() / 8) as usize;
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
            if region.source.contains(&offset.get()) {
                if let Some(moved) =
                    NonZeroU64::new(offset.get() - region.source.start + region.destination)
                {
                    return moved;
                }
            }
        }
        unreachable!("internal: a moved handle names an object in no occupied region")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::space::FIRST_OBJECT;

    #[test]
    fn set_live_across_a_chunk_boundary_sets_exactly_those_bits() {
        let mut table = ForwardingTable::with_words(192, FIRST_OBJECT / 8);
        table.set_live(60..70);
        assert_eq!(table.chunks[0].live, 0b1111 << 60);
        assert_eq!(table.chunks[1].live, 0b11_1111);
        assert_eq!(table.chunks[2].live, 0);
        table.set_live(128..192);
        assert_eq!(table.chunks[2].live, u64::MAX, "a whole chunk at once");
        assert_eq!(table.summarize(0), 10 + 64);
    }

    #[test]
    fn the_old_segment_keeps_the_bottom_and_the_nursery_follows_it() {
        // Base 128 words, the old segment from chunk 2 on; one young object
        // at words 10..13 and two old ones at 130..132 and 200..204.
        let mut table = ForwardingTable::with_words(256, 128);
        table.set_live(10..13);
        table.set_live(130..132);
        table.set_live(200..204);
        assert_eq!(table.summarize(2), 3 + 2 + 4);
        assert_eq!(table.destination(130), 128);
        assert_eq!(table.destination(200), 128 + 2);
        assert_eq!(table.destination(10), 128 + 6, "after every old survivor");
        assert_eq!(table.destination(12), 128 + 8);
    }

    #[test]
    fn the_first_live_word_goes_to_the_first_object() {
        let mut table = ForwardingTable::with_words(256, FIRST_OBJECT / 8);
        table.set_live(100..104);
        table.summarize(0);
        assert_eq!(table.destination(100), FIRST_OBJECT / 8);
    }

    #[test]
    fn a_word_after_a_dead_gap_goes_past_the_live_words_before_it() {
        let mut table = ForwardingTable::with_words(256, FIRST_OBJECT / 8);
        table.set_live(2..5);
        table.set_live(62..66);
        table.set_live(200..203);
        table.summarize(0);
        assert_eq!(table.destination(62), FIRST_OBJECT / 8 + 3);
        assert_eq!(table.destination(65), FIRST_OBJECT / 8 + 3 + 3);
        assert_eq!(table.destination(200), FIRST_OBJECT / 8 + 3 + 4);
        assert!(!table.is_live(5));
        assert!(table.is_live(202));
    }
}
