//! Each application's instances dispatch id, by application index, in a
//! reservation machine code reads (`docs/design/execution.md`, "Sends in
//! machine code"): the megamorphic stub finds a `super` or private
//! lexical send's key -- S's or N's application, which its cell compares
//! as the class object's application word -- and a lexical send's rule
//! payload, N's id, here, so that it files the send under the dispatch id
//! the global cache is keyed by, as the interpreter does.
//!
//! The table is a copy of what the application table records, written
//! wherever a row of it is: `Heap::register_application` writes the id
//! the application was minted, `Heap::free_application` the `NONE` of a
//! row that holds none, and `Heap::truncate_application_rows` takes the
//! bound back. An application's ids never change once it is registered,
//! and which application a class object names
//! (`Heap::class_set_application`) is the class object's application
//! word, which the stub reads afresh, so nothing else writes it.
//!
//! The bound, `MachineWord::ApplicationIdRows`, is the number of rows
//! recorded, so that machine code never reads past what is committed: an
//! index at or past it -- none a class object of this heap holds, unless
//! the table ran out of room -- is a miss, which the miss leaf decides.
//!
//! Aliasing: machine code reaches the rows through the reservation's own
//! pointer (`Reservation::base_address`), while Rust writes them through
//! a slice each write borrows afresh and drops before it returns, as the
//! cells' (`interpreter/inline_cache/cells.rs`).

use ns_memory::Reservation;

use crate::mixin::DispatchId;

/// The bytes of one row: the id, zero-extended to a word.
const ROW_BYTES: usize = 8;

/// The most rows the table records: room for 4,194,304 applications, 32
/// MiB of address space, past the ids the header word's 22-bit guard
/// field can mint (`DispatchId::LIMIT`), so that only applications
/// without an id of their own fall past it.
const CAPACITY_ROWS: usize = 1 << 22;

/// The least a growth commits.
const MINIMUM_GROWTH: usize = 64 << 10;

/// The table: a reservation of `CAPACITY_ROWS` rows, committed as it
/// grows, and how many rows are recorded.
pub(crate) struct ApplicationIds {
    reservation: Reservation,
    committed: usize,
    rows: usize,
}

impl ApplicationIds {
    /// Reserves the table, committing none of it. A refusal is running
    /// out of memory, as the object space's is.
    pub(crate) fn new() -> ApplicationIds {
        let bytes = CAPACITY_ROWS * ROW_BYTES;
        let reservation =
            Reservation::reserve(bytes).unwrap_or_else(|_| super::out_of_memory(bytes));
        ApplicationIds {
            reservation,
            committed: 0,
            rows: 0,
        }
    }

    /// The address of row 0, fixed for the reservation's life.
    pub(crate) fn address(&self) -> usize {
        self.reservation.base_address()
    }

    /// How many rows are recorded: the bound machine code compares an
    /// index with.
    pub(crate) fn rows(&self) -> usize {
        self.rows
    }

    /// Records `id` for row `row` of a table of `table_rows` rows, and the
    /// bound the table's length sets; a row past the capacity is not
    /// recorded, and the bound stays at the capacity.
    pub(crate) fn record(&mut self, row: usize, id: DispatchId, table_rows: usize) {
        self.set_rows(table_rows);
        if row >= self.rows {
            return;
        }
        let offset = row * ROW_BYTES;
        if let Some(words) = self.reservation.words_mut(offset..offset + ROW_BYTES) {
            words[0] = u64::from(id.bits());
        }
    }

    /// Takes the bound to what a table of `table_rows` rows records,
    /// committing the rows it newly covers, which read as `NONE` -- 0 --
    /// until a registration writes them.
    pub(crate) fn set_rows(&mut self, table_rows: usize) {
        let rows = table_rows.min(CAPACITY_ROWS);
        let end = rows * ROW_BYTES;
        if end > self.committed {
            let page = Reservation::page_size();
            let needed = end.div_ceil(page) * page;
            let generous = (self.committed + self.committed.max(MINIMUM_GROWTH))
                .div_ceil(page)
                .saturating_mul(page)
                .min(CAPACITY_ROWS * ROW_BYTES);
            let target = needed.max(generous);
            let from = self.committed;
            self.committed = if self.reservation.commit(from..target).is_ok() {
                target
            } else if self.reservation.commit(from..needed).is_ok() {
                needed
            } else {
                super::out_of_memory(needed - self.committed)
            };
        }
        self.rows = rows;
    }

    /// The id row `row` records; `None` past the bound.
    #[cfg(all(test, debug_assertions))]
    pub(crate) fn id(&self, row: usize) -> Option<u64> {
        if row >= self.rows {
            return None;
        }
        let offset = row * ROW_BYTES;
        self.reservation
            .words(offset..offset + ROW_BYTES)
            .map(|words| words[0])
    }
}
