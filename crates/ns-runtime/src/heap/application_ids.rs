//! What machine code reads of each application, by application index, in
//! a reservation of its own (`docs/design/execution.md`, "Sends in machine
//! code", "Stage 4: closures, non-local return, allocation and the
//! barrier"): a row of four words (`RowWord`).
//!
//! - The instances dispatch id. The megamorphic stub finds a `super` or
//!   private lexical send's key -- S's or N's application, which its cell
//!   compares as the class object's application word -- and a lexical
//!   send's rule payload, N's id, here, so that it files the send under
//!   the dispatch id the global cache is keyed by, as the interpreter
//!   does.
//! - The instance header: the header word of an ordinary instance, which
//!   `AllocateInstance`'s template writes, or 0 for an application whose
//!   instances it does not make (`Row::instance_header`).
//! - The own slot base (`MixinApplication::own_slot_base`), from which an
//!   own-slot instruction addresses the slots of the level its frame was
//!   entered at, as `Heap::activation_slot_base` does.
//! - The flags (`ROW_FLAG_VALUE_SLOTS`).
//!
//! The table is a copy of what the application table records, written
//! wherever a row of it is: `Heap::register_application` writes the row
//! of the application it registers, `Heap::free_application` the zeros
//! of a row that holds none -- the id `NONE` -- and
//! `Heap::truncate_application_rows` takes the bound back. An
//! application's row never changes once it is registered, and which
//! application a class object names (`Heap::class_set_application`) is
//! the class object's application word, which machine code reads afresh,
//! so nothing else writes it.
//!
//! The bound, `MachineWord::ApplicationIdRows`, is the number of rows
//! recorded, so that machine code never reads past what is committed: an
//! index at or past it -- none a class object of this heap holds, unless
//! the table ran out of room -- is a miss, which the miss leaf decides.
//! A full collection gives the pages past the bound and a headroom back
//! once the truncation lowered it (`ApplicationIds::shrink`).
//!
//! Aliasing: machine code reaches the rows through the reservation's own
//! pointer (`Reservation::base_address`), while Rust writes them through
//! a slice each write borrows afresh and drops before it returns, as the
//! cells' (`interpreter/inline_cache/cells.rs`).

use ns_memory::Reservation;

use crate::mixin::DispatchId;

/// The words of a row, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RowWord {
    /// The instances dispatch id, zero-extended.
    DispatchId = 0,
    /// The instance header word, or 0 (`Row::instance_header`).
    InstanceHeader = 1,
    /// The own slot base, a plain number.
    SlotBase = 2,
    /// The flags (`ROW_FLAG_VALUE_SLOTS`).
    Flags = 3,
}

#[cfg(ns_jit)]
impl RowWord {
    /// The word's displacement from its row's address.
    pub(crate) const fn displacement(self) -> i32 {
        self as i32 * 8
    }
}

/// The words of a row.
const ROW_WORDS: usize = 4;

/// How far an application index is shifted to be its row's byte offset.
pub(crate) const ROW_SHIFT: u8 = 5;

/// The bytes of one row.
const ROW_BYTES: usize = ROW_WORDS * 8;

// `Row::words` writes the words in this order.
const _: () = assert!(
    1 << ROW_SHIFT == ROW_BYTES
        && RowWord::DispatchId as usize == 0
        && RowWord::InstanceHeader as usize == 1
        && RowWord::SlotBase as usize == 2
        && RowWord::Flags as usize == ROW_WORDS - 1
);

/// The flag of an application whose instances' slots take §3.1.1's
/// check of what a value's slot may hold (`instantiate::check_value_slot`):
/// the application inherits from `Value`
/// (`MixinApplication::inherits_value`).
pub(crate) const ROW_FLAG_VALUE_SLOTS: u64 = 1;

/// What a row records of an application -- or of none, the zeros.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Row {
    /// The instances dispatch id.
    pub(crate) dispatch_id: DispatchId,
    /// The header word an instance is born with
    /// (`MixinApplication::instance_header`), or 0 where
    /// `allocate_for_factory` makes no ordinary instance of the
    /// application's slot count by it: the application refuses
    /// instantiation, or its count needs an overflow word. What depends
    /// on the class object rather than the application -- a builtin's own
    /// way of making its instances, the value class refusal -- the
    /// template reads off the class object afresh at every instantiation.
    pub(crate) instance_header: u64,
    /// `MixinApplication::own_slot_base`.
    pub(crate) slot_base: usize,
    /// `MixinApplication::inherits_value`.
    pub(crate) value_slots: bool,
}

impl Row {
    /// The row's words, in `RowWord` order.
    fn words(self) -> [u64; ROW_WORDS] {
        [
            u64::from(self.dispatch_id.bits()),
            self.instance_header,
            self.slot_base as u64,
            if self.value_slots {
                ROW_FLAG_VALUE_SLOTS
            } else {
                0
            },
        ]
    }
}

/// The most rows the table records: room for 4,194,304 applications, 128
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
    /// How far the next shrink goes (`shrink`).
    damping: super::limits::ShrinkDamping,
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
            damping: super::limits::ShrinkDamping::default(),
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

    /// How many rows are committed, recorded or not.
    pub(crate) fn committed_rows(&self) -> usize {
        self.committed / ROW_BYTES
    }

    /// Records `recorded` as row `row` of a table of `table_rows` rows, and
    /// the bound the table's length sets; a row past the capacity is not
    /// recorded, and the bound stays at the capacity.
    pub(crate) fn record(&mut self, row: usize, recorded: Row, table_rows: usize) {
        self.set_rows(table_rows);
        if row >= self.rows {
            return;
        }
        let offset = row * ROW_BYTES;
        if let Some(words) = self.reservation.words_mut(offset..offset + ROW_BYTES) {
            words.copy_from_slice(&recorded.words());
        }
    }

    /// Takes the bound to what a table of `table_rows` rows records,
    /// committing the rows it newly covers, which read as zeros -- the id
    /// `NONE` -- until a registration writes them.
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

    /// Gives back the committed pages above the recorded rows and
    /// `shrink`'s headroom once more than its threshold lies above them
    /// (`limits::Shrink`), and answers how many bytes that was. Run after
    /// a full collection, once `Heap::truncate_application_rows` has taken
    /// the bound -- the rows recorded, which machine code compares an index
    /// with -- back to the application table's length: the bound lies at
    /// or below the high mark, so no page machine code reads goes.
    pub(crate) fn shrink(&mut self, shrink: super::limits::Shrink, damped: bool) -> usize {
        let Some(end) = shrink.keep(
            &mut self.damping,
            damped,
            self.rows * ROW_BYTES,
            self.committed,
        ) else {
            return 0;
        };
        let from = self.committed;
        self.committed = end;
        match self.reservation.decommit(end..from) {
            Ok(()) => from - end,
            // Counted uncommitted, the next growth commits them again.
            Err(_) => 0,
        }
    }

    /// The words row `row` records; `None` past the bound.
    #[cfg(all(test, debug_assertions))]
    pub(crate) fn row_words(&self, row: usize) -> Option<[u64; ROW_WORDS]> {
        if row >= self.rows {
            return None;
        }
        let offset = row * ROW_BYTES;
        self.reservation
            .words(offset..offset + ROW_BYTES)
            .and_then(|words| <[u64; ROW_WORDS]>::try_from(words).ok())
    }

    /// The words `recorded` writes into a row, for a check of the table.
    #[cfg(all(test, debug_assertions))]
    pub(crate) fn expected_words(recorded: Row) -> [u64; ROW_WORDS] {
        recorded.words()
    }
}
