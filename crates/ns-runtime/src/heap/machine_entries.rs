//! Where machine code enters each block's body, by the body's code id, in
//! a reservation of its own (`docs/design/execution.md`, "Stage 4:
//! closures, non-local return, allocation and the barrier"): the closure
//! stubs (`jit::targets`) read the row a closure's code id names, and
//! jump to its machine entry, as a cell names a method's.
//!
//! A row is four words (`RowWord`): the body's machine entry, the address
//! of its first byte in the code zone's executable view, 0 for none; the
//! feedback base its machine code was compiled against, as the
//! `SmallInteger` word a closure holds it in, which a closure's must
//! equal, since the body's sends address the cells of that base's range;
//! the body's parameter count, which a call's arity must match; and a
//! word of padding, so that a row's offset is its id shifted.
//!
//! A row names machine code only while three things hold: the code id
//! names the body (a full collection frees an id nothing names and mints
//! it again for another body), the body is compiled into the zone and not
//! abandoned, and the zone is the one the row's address lies in. So every
//! row is a copy the JIT writes from what it knows of the body
//! (`jit::note_block_entry`), and each way one of the three stops holding
//! clears it: the heap's free of an unnamed code id
//! (`Heap::free_unnamed_codes`), the JIT's abandonment of a body
//! (`jit::abandon`), and the replacement of the zone
//! (`Interpreter::set_jit_zone_bytes`), which clears every row, as a switch
//! of the JIT on or off does (`Interpreter::set_jit_enabled`), since a
//! body pushed while it is off is refused without being abandoned. A body
//! is never compiled into the same zone twice, so a row's entry, once
//! written, changes only by being cleared.
//!
//! The bound, `MachineWord::MachineEntryRows`, is the number of rows
//! committed, so that machine code never reads past what is committed: an
//! id at or past it reads as no machine entry.
//!
//! Aliasing: machine code reaches the rows through the reservation's own
//! pointer (`Reservation::base_address`), while Rust writes them through a
//! slice each write borrows afresh and drops before it returns, as the
//! cells' (`interpreter/inline_cache/cells.rs`).

use ns_memory::Reservation;

/// The words of a row, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RowWord {
    /// The body's machine entry, or 0.
    Entry = 0,
    /// The feedback base the body was compiled against, as a closure's
    /// `SmallInteger` word holds it.
    FeedbackBase = 1,
    /// The body's parameter count, a plain number.
    Parameters = 2,
}

#[cfg(ns_jit)]
impl RowWord {
    /// The word's displacement from its row's address.
    pub(crate) const fn displacement(self) -> i32 {
        self as i32 * 8
    }
}

/// The words of a row: `RowWord`'s three and the padding.
pub(crate) const ROW_WORDS: usize = 4;

/// How far a code id is shifted to be its row's byte offset.
pub(crate) const ROW_SHIFT: u8 = 5;

/// The bytes of one row.
const ROW_BYTES: usize = ROW_WORDS * 8;

const _: () = assert!(
    1 << ROW_SHIFT == ROW_BYTES
        && RowWord::Entry as usize == 0
        && RowWord::FeedbackBase as usize == 1
        && RowWord::Parameters as usize == 2
);

/// The most rows the table records: room for 1,048,576 code ids, 32 MiB of
/// address space. A body whose id lies past it is called by the
/// interpreter alone.
const CAPACITY_ROWS: usize = 1 << 20;

/// The least a growth commits.
const MINIMUM_GROWTH: usize = 64 << 10;

/// The table: a reservation of `CAPACITY_ROWS` rows, committed as rows
/// are written, and how many rows are committed.
pub(crate) struct MachineEntries {
    reservation: Reservation,
    /// How many rows are committed, and so read.
    rows: usize,
    /// One bit per row, set while the row names a machine entry: what the
    /// interpreter asks on every `value` send of a block
    /// (`Heap::closure_has_machine_entry`), answered without the commit
    /// check a read of the reservation makes. Rust's alone, kept by every
    /// write of a row.
    named_entries: Vec<u64>,
}

impl MachineEntries {
    /// Reserves the table, committing none of it. A refusal is running out
    /// of memory, as the object space's is.
    pub(crate) fn new() -> MachineEntries {
        let bytes = CAPACITY_ROWS * ROW_BYTES;
        let reservation =
            Reservation::reserve(bytes).unwrap_or_else(|_| super::out_of_memory(bytes));
        MachineEntries {
            reservation,
            rows: 0,
            named_entries: Vec::new(),
        }
    }

    /// The address of row 0, fixed for the reservation's life.
    #[cfg_attr(
        not(ns_jit),
        expect(dead_code, reason = "only machine code reads the rows")
    )]
    pub(crate) fn address(&self) -> usize {
        self.reservation.base_address()
    }

    /// How many rows are committed: the bound machine code compares an id
    /// with.
    pub(crate) fn rows(&self) -> usize {
        self.rows
    }

    /// The words of row `row`, zeros for one past the bound.
    pub(crate) fn row(&self, row: usize) -> [u64; ROW_WORDS] {
        if row >= self.rows {
            return [0; ROW_WORDS];
        }
        let offset = row * ROW_BYTES;
        self.reservation
            .words(offset..offset + ROW_BYTES)
            .and_then(|words| <[u64; ROW_WORDS]>::try_from(words).ok())
            .unwrap_or([0; ROW_WORDS])
    }

    /// Whether row `row` names a machine entry: `row(row)`'s entry word is
    /// not 0.
    pub(crate) fn names_entry(&self, row: usize) -> bool {
        self.named_entries
            .get(row / 64)
            .is_some_and(|bits| bits & (1 << (row % 64)) != 0)
    }

    /// Sets or clears row `row`'s bit of `named_entries`.
    fn mark_names_entry(&mut self, row: usize, names_entry: bool) {
        let word = row / 64;
        if names_entry && word >= self.named_entries.len() {
            self.named_entries.resize(word + 1, 0);
        }
        if let Some(bits) = self.named_entries.get_mut(word) {
            if names_entry {
                *bits |= 1 << (row % 64);
            } else {
                *bits &= !(1 << (row % 64));
            }
        }
    }

    /// Writes `words` into row `row`, committing the rows up to it first;
    /// a row past the capacity is not written. Answers whether the bound
    /// grew.
    pub(crate) fn record(&mut self, row: usize, words: [u64; ROW_WORDS]) -> bool {
        if row >= CAPACITY_ROWS {
            return false;
        }
        let grew = row >= self.rows;
        if grew {
            self.commit_through(row);
        }
        let offset = row * ROW_BYTES;
        if let Some(slots) = self.reservation.words_mut(offset..offset + ROW_BYTES) {
            slots.copy_from_slice(&words);
            self.mark_names_entry(row, words[RowWord::Entry as usize] != 0);
        }
        grew
    }

    /// Clears row `row`, if it is committed.
    pub(crate) fn clear(&mut self, row: usize) {
        if row < self.rows {
            let offset = row * ROW_BYTES;
            if let Some(slots) = self.reservation.words_mut(offset..offset + ROW_BYTES) {
                slots.fill(0);
            }
            self.mark_names_entry(row, false);
        }
    }

    /// Clears every committed row.
    pub(crate) fn clear_all(&mut self) {
        if let Some(slots) = self.reservation.words_mut(0..self.rows * ROW_BYTES) {
            slots.fill(0);
        }
        self.named_entries.fill(0);
    }

    /// Commits the rows through `row`, and a generous margin past it, which
    /// read as zeros -- no machine entry -- until a row is written.
    fn commit_through(&mut self, row: usize) {
        let committed = self.rows * ROW_BYTES;
        let page = Reservation::page_size();
        let needed = ((row + 1) * ROW_BYTES).div_ceil(page) * page;
        let generous = (committed + committed.max(MINIMUM_GROWTH))
            .div_ceil(page)
            .saturating_mul(page);
        let target = needed.max(generous).min(CAPACITY_ROWS * ROW_BYTES);
        let end = if self.reservation.commit(committed..target).is_ok() {
            target
        } else if self.reservation.commit(committed..needed).is_ok() {
            needed
        } else {
            super::out_of_memory(needed - committed)
        };
        self.rows = end / ROW_BYTES;
    }
}

#[cfg(test)]
mod tests {
    use super::{MachineEntries, RowWord, ROW_WORDS};

    /// What `names_entry` answers for every committed row and one past
    /// them, against the row's own entry word.
    fn assert_bits_match_rows(entries: &MachineEntries) {
        for row in 0..=entries.rows() {
            assert_eq!(
                entries.names_entry(row),
                entries.row(row)[RowWord::Entry as usize] != 0,
                "row {row}"
            );
        }
    }

    /// The bit the interpreter's `value` send asks follows every write of
    /// a row -- a record with an entry and one without, a clear, a clear of
    /// them all -- so that it never says a block has machine code its row
    /// does not name, nor misses one it does.
    #[test]
    fn the_names_entry_bit_follows_every_write_of_a_row() {
        let mut entries = MachineEntries::new();
        assert!(!entries.names_entry(0));
        let words = |entry: u64| -> [u64; ROW_WORDS] { [entry, 8, 1, 0] };
        entries.record(3, words(0x1000));
        entries.record(70, words(0x2000));
        entries.record(5, words(0));
        assert!(entries.names_entry(3) && entries.names_entry(70));
        assert_bits_match_rows(&entries);
        entries.record(3, words(0));
        assert!(!entries.names_entry(3));
        entries.clear(70);
        assert!(!entries.names_entry(70));
        assert_bits_match_rows(&entries);
        entries.record(9, words(0x3000));
        entries.clear_all();
        assert!(!entries.names_entry(9));
        assert_bits_match_rows(&entries);
    }
}
