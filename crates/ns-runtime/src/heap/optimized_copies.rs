//! The dispatching rows of each method body's optimized copies, by the
//! body's code id, in a reservation of its own (`docs/design/execution.md`,
//! "Speculation", "The dispatching entry"): what the dispatching stub
//! (`jit::targets`) compares a `super` or private lexical send's
//! receiver and current class with, to enter the copy customized for
//! both. Such a send's cell is guarded by S's or N's application word,
//! which says nothing of the receiver, so the cell names the body's
//! dispatching entry rather than a copy.
//!
//! A body's block is `DISPATCHING_ROWS` rows of three words (`RowWord`):
//! the receiver's dispatch id the copy was customized for, the
//! application word (`Heap::class_application_word`) of the class it is
//! entered at, and the copy's machine entry. A row past the body's copies
//! is vacant: its receiver word is `VACANT`, which no dispatch id is, so
//! that the stub compares no row it must test for vacancy first. Block 0
//! belongs to no code id and stays vacant for the reservation's life: a
//! body's dispatching data word names it until the JIT points the word at
//! the block of the body's code id (`jit::point_budget_word`), so that the
//! stub falls through to the body's baseline code with nothing to test.
//!
//! The machine code reaching a block is the body's own dispatching entry,
//! through its data word, so a block's contents matter only while a data
//! word names it. A full collection may free a code id and mint it again
//! for another body, after which every body's word is pointed afresh and
//! its block written from its copies before any code runs; a block no
//! word names is left as it is, and written in full when one names it
//! again. A block is written only from the copies of the body whose word
//! names it (`JitState::write_dispatching_rows`), and afresh wherever a
//! copy is installed, freed or moved.
//!
//! Aliasing: machine code reaches the rows through the data word's
//! address, which lies in the reservation, while Rust writes them through
//! a slice each write borrows afresh and drops before it returns, as the
//! machine entry table's (`machine_entries.rs`).

use ns_memory::Reservation;

/// The rows a body's block holds: a copy past them is entered by a
/// receiver-guarded cell, the push, and the call stub only through the
/// key's lookup on the Rust side, which an interpreted send into a body
/// with more installed copies than rows takes (`jit::called`); one into a
/// body with no more enters the dispatching entry.
pub(crate) const DISPATCHING_ROWS: usize = 8;

/// The words of a row, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RowWord {
    /// The receiver's dispatch id the copy was customized for, or
    /// `VACANT`.
    Receiver = 0,
    /// The application word of the class the copy is entered at.
    Entered = 1,
    /// The copy's machine entry.
    Entry = 2,
}

impl RowWord {
    /// The word's displacement from its row's address.
    pub(crate) const fn displacement(self) -> i32 {
        self as i32 * 8
    }
}

/// The words of a row.
pub(crate) const ROW_WORDS: usize = 3;

/// The bytes of a row, as machine code steps from one to the next.
pub(crate) const ROW_BYTES: i32 = RowWord::Entry.displacement() + 8;

const _: () = assert!(
    RowWord::Receiver as usize == 0
        && RowWord::Entered as usize == 1
        && RowWord::Entry as usize == 2
        && ROW_WORDS == 3
        && ROW_BYTES == 24
);

/// The words of a block.
const BLOCK_WORDS: usize = DISPATCHING_ROWS * ROW_WORDS;

/// The bytes of a block.
const BLOCK_BYTES: usize = BLOCK_WORDS * 8;

/// The receiver word of a vacant row: no dispatch id, which a header
/// holds in 22 bits, is all ones.
pub(crate) const VACANT: u64 = u64::MAX;

/// The most code ids the table holds blocks for: 262,144, 48 MiB of
/// address space. A body whose id lies past it is entered at its baseline
/// code by its dispatching entry, its word naming the vacant block.
const CAPACITY_IDS: usize = 1 << 18;

/// The least a growth commits.
const MINIMUM_GROWTH: usize = 64 << 10;

/// One row's words.
pub(crate) type Row = [u64; ROW_WORDS];

/// A vacant row's words.
const VACANT_ROW: Row = [VACANT, 0, 0];

/// The table: a reservation of `CAPACITY_IDS` blocks past the vacant
/// block 0, committed as blocks are pointed at, and given back above the
/// code table's ids after a full collection (`shrink`).
pub(crate) struct OptimizedCopies {
    reservation: Reservation,
    /// The bytes committed, a multiple of the page.
    committed: usize,
    /// How many whole blocks the committed bytes hold, block 0 among them.
    blocks: usize,
    /// How far the next shrink goes (`shrink`).
    damping: super::limits::ShrinkDamping,
    /// Whether it committed more since the last shrink: demand, which
    /// keeps the next collection from shrinking it.
    grew: bool,
}

impl OptimizedCopies {
    /// Reserves the table and commits its vacant block. A refusal is
    /// running out of memory, as the object space's is.
    pub(crate) fn new() -> OptimizedCopies {
        let bytes = (CAPACITY_IDS + 1) * BLOCK_BYTES;
        let reservation =
            Reservation::reserve(bytes).unwrap_or_else(|_| super::out_of_memory(bytes));
        let mut table = OptimizedCopies {
            reservation,
            committed: 0,
            blocks: 0,
            damping: super::limits::ShrinkDamping::default(),
            grew: false,
        };
        table.commit_through(0);
        table.grew = false;
        table
    }

    /// The bytes committed, which `Interpreter::table_sizes` reports.
    pub(crate) fn committed_bytes(&self) -> usize {
        self.committed
    }

    /// Whether the block at `address` -- the vacant block, or one
    /// `block_address` answered -- lies within what is committed. Only a
    /// debug build's check of the dispatching words after a shrink asks.
    #[cfg(any(test, debug_assertions))]
    pub(crate) fn is_committed(&self, address: u64) -> bool {
        usize::try_from(address)
            .ok()
            .and_then(|address| address.checked_sub(self.reservation.base_address()))
            .is_some_and(|offset| offset + BLOCK_BYTES <= self.committed)
    }

    /// Gives back the committed pages above the blocks of `high_ids` code
    /// ids and `shrink`'s headroom once more than its threshold lies above
    /// them (`limits::Shrink`), and answers how many bytes that was.
    /// `high_ids` is the code table's length after a full collection
    /// truncated it, called once every body's word was pointed afresh
    /// (`jit::point_budget_words_after_collection`): a live body's names
    /// the block of an id below it, a dead one's the vacant block, so no
    /// word names a block given back.
    pub(crate) fn shrink(
        &mut self,
        high_ids: usize,
        shrink: super::limits::Shrink,
        damped: bool,
    ) -> usize {
        let high_mark = (high_ids.min(CAPACITY_IDS) + 1) * BLOCK_BYTES;
        let Some(end) = shrink.keep(
            &mut self.damping,
            damped,
            std::mem::take(&mut self.grew),
            high_mark.min(self.committed),
            self.committed,
        ) else {
            return 0;
        };
        let committed = self.committed;
        self.committed = end;
        self.blocks = end / BLOCK_BYTES;
        match self.reservation.decommit(end..committed) {
            Ok(()) => committed - end,
            // Counted uncommitted, the next growth commits them again.
            Err(_) => 0,
        }
    }

    /// The address of the vacant block, which matches no receiver.
    pub(crate) fn vacant_address(&self) -> u64 {
        self.reservation.base_address() as u64
    }

    /// The address of code id `id`'s block, committed: the vacant block's
    /// for an id past the capacity.
    pub(crate) fn block_address(&mut self, id: usize) -> u64 {
        if id >= CAPACITY_IDS {
            return self.vacant_address();
        }
        let block = id + 1;
        if block >= self.blocks {
            self.commit_through(block);
        }
        (self.reservation.base_address() + block * BLOCK_BYTES) as u64
    }

    /// The block at `address`, one `block_address` answered for a code
    /// id; `None` for the vacant block and any other address.
    fn block_at(&self, address: u64) -> Option<usize> {
        let offset = usize::try_from(address)
            .ok()?
            .checked_sub(self.reservation.base_address())?;
        let block = offset / BLOCK_BYTES;
        (offset % BLOCK_BYTES == 0 && block != 0 && block < self.blocks).then_some(block)
    }

    /// Writes `rows`, at most `DISPATCHING_ROWS` of them, into the block at
    /// `address`, the rest vacant; the vacant block is never written.
    pub(crate) fn write(&mut self, address: u64, rows: &[Row]) {
        let Some(block) = self.block_at(address) else {
            return;
        };
        let offset = block * BLOCK_BYTES;
        let Some(words) = self.reservation.words_mut(offset..offset + BLOCK_BYTES) else {
            unreachable!("internal: a committed block is read-write");
        };
        for (index, slot) in words.as_chunks_mut::<ROW_WORDS>().0.iter_mut().enumerate() {
            *slot = rows.get(index).copied().unwrap_or(VACANT_ROW);
        }
    }

    /// The rows of the block at `address` that are not vacant, in order.
    pub(crate) fn rows(&self, address: u64) -> Vec<Row> {
        let Some(block) = self.block_at(address) else {
            return Vec::new();
        };
        let offset = block * BLOCK_BYTES;
        self.reservation
            .words(offset..offset + BLOCK_BYTES)
            .map(|words| {
                words
                    .as_chunks::<ROW_WORDS>()
                    .0
                    .iter()
                    .filter(|row| row[RowWord::Receiver as usize] != VACANT)
                    .copied()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Makes every committed block vacant: the code zone the rows' entries
    /// lie in was replaced.
    pub(crate) fn clear_all(&mut self) {
        let committed = self.blocks * BLOCK_BYTES;
        if let Some(words) = self.reservation.words_mut(0..committed) {
            words.as_chunks_mut::<ROW_WORDS>().0.fill(VACANT_ROW);
        }
    }

    /// Commits the blocks through `block`, and a generous margin past it,
    /// each vacant.
    fn commit_through(&mut self, block: usize) {
        self.grew = true;
        let committed = self.committed;
        let page = Reservation::page_size();
        let capacity = ((CAPACITY_IDS + 1) * BLOCK_BYTES).div_ceil(page) * page;
        let needed = ((block + 1) * BLOCK_BYTES).div_ceil(page) * page;
        let generous = (committed + committed.max(MINIMUM_GROWTH)).div_ceil(page) * page;
        let target = needed.max(generous).min(capacity);
        let end = if self.reservation.commit(committed..target).is_ok() {
            target
        } else if self.reservation.commit(committed..needed).is_ok() {
            needed
        } else {
            super::out_of_memory(needed - committed)
        };
        let blocks = end / BLOCK_BYTES;
        let (start, stop) = (self.blocks * BLOCK_BYTES, blocks * BLOCK_BYTES);
        if let Some(words) = self.reservation.words_mut(start..stop) {
            words.as_chunks_mut::<ROW_WORDS>().0.fill(VACANT_ROW);
        }
        self.committed = end;
        self.blocks = blocks;
    }
}

#[cfg(test)]
mod tests {
    use super::{OptimizedCopies, BLOCK_BYTES, DISPATCHING_ROWS, VACANT};
    use crate::heap::limits::Shrink;

    /// After a full collection the table gives back what lies above the
    /// code table's ids and the headroom, once more than the threshold
    /// does and it grew no more since the last shrink, keeping the vacant
    /// block and every block below the mark with its rows; a later
    /// pointing past it commits the blocks again, vacant.
    #[test]
    fn the_table_gives_back_the_blocks_above_the_code_table() {
        let page = ns_memory::Reservation::page_size();
        let pair = Shrink {
            threshold: page,
            headroom: page,
        };
        let mut table = OptimizedCopies::new();
        let kept = table.block_address(2);
        table.write(kept, &[[11, 3, 0x3000]]);
        let far = table.block_address(20_000);
        table.write(far, &[[12, 3, 0x4000]]);
        let grown = table.committed_bytes();
        assert!(grown >= 20_001 * BLOCK_BYTES);
        assert_eq!(table.shrink(3, pair, false), 0, "a growth since keeps it");
        let given_back = table.shrink(3, pair, false);
        assert!(given_back > 0);
        assert_eq!(table.committed_bytes(), grown - given_back);
        assert!(table.committed_bytes() <= (4 * BLOCK_BYTES + page).next_multiple_of(page));
        assert!(table.is_committed(table.vacant_address()) && table.is_committed(kept));
        assert!(!table.is_committed(far));
        assert_eq!(table.rows(kept), vec![[11, 3, 0x3000]]);
        assert!(table.rows(far).is_empty(), "given back");
        let again = table.block_address(20_000);
        assert_eq!(again, far);
        assert!(table.rows(again).is_empty(), "committed again, vacant");
        assert_eq!(table.shrink(3, pair, false), 0, "grown again");
    }

    /// A block holds what was last written into it, its other rows
    /// vacant, and the vacant block is never written: a body's word that
    /// names it until it is pointed matches no receiver.
    #[test]
    fn a_block_holds_its_last_rows_and_the_vacant_block_none() {
        let mut table = OptimizedCopies::new();
        let vacant = table.vacant_address();
        table.write(vacant, &[[7, 9, 0x1000]]);
        assert!(table.rows(vacant).is_empty());
        let block = table.block_address(3);
        assert_ne!(block, vacant);
        assert!(table.rows(block).is_empty(), "committed vacant");
        let rows: Vec<[u64; 3]> = (0..DISPATCHING_ROWS as u64 + 2)
            .map(|row| [row + 10, 1, 0x2000 + row])
            .collect();
        table.write(block, &rows);
        assert_eq!(table.rows(block), rows[..DISPATCHING_ROWS].to_vec());
        table.write(block, &rows[..1]);
        assert_eq!(table.rows(block), rows[..1].to_vec());
        let far = table.block_address(5000);
        table.write(far, &rows[..2]);
        assert_eq!(table.rows(far), rows[..2].to_vec());
        assert_eq!(table.rows(block), rows[..1].to_vec());
        table.clear_all();
        assert!(table.rows(block).is_empty() && table.rows(far).is_empty());
        assert_ne!(rows[0][0], VACANT);
    }
}
