//! Each body's optimization budget by its code id, in a reservation of
//! its own (`docs/design/execution.md`, "When a body is optimized"):
//! baseline machine code subtracts the body's length at each entry and the
//! jump's distance at each back jump from the body's row, and once the
//! row falls below zero calls the budget leaf (`jit::leaves`), which
//! records the body's optimization request and parks the row.
//!
//! A row is one signed word, the instructions the body may still run
//! before it is optimized. Beside the rows, Rust keeps one bit per row,
//! set while the row is armed -- written for the body its code id names
//! (`arm`), and not cleared since -- so that the leaf tells a budget that
//! ran out from a row that was cleared under the code that spends it.
//!
//! Machine code never names a row by its code id, which a full collection
//! may free and mint again for another body: each copy of a body's
//! baseline code keeps the address of its row in a data word of its own
//! (`jit::baseline`), which the JIT points at the row once the code is
//! installed, and afresh after every full collection, at the row of the
//! id the body has then (`jit::point_budget_words_after_collection`); a
//! word still 0 asks the leaf to point it at its first spend. A code id
//! past `CAPACITY_ROWS` has no row: its copies spend from the sink, one
//! more word past the rows, parked so that it never runs out in
//! practice.
//!
//! The rows are committed on demand, as `machine_entries.rs`'s are, and a
//! full collection gives the pages above the code table's length and a
//! headroom back (`shrink`), after every data word was pointed at a row
//! of an id the table holds, so that no machine code reads a page given
//! back.
//!
//! Aliasing: machine code reaches the rows through the reservation's own
//! pointer (`Reservation::base_address`), while Rust writes them through a
//! slice each write borrows afresh and drops before it returns, as the
//! machine entry table's.

use ns_memory::Reservation;

use super::CodeId;

/// The most rows the table records: room for 1,048,576 code ids, 8 MiB of
/// address space, as many as the machine entry table holds.
const CAPACITY_ROWS: usize = 1 << 20;

/// The bytes of one row.
const ROW_BYTES: usize = 8;

/// The least a growth commits.
const MINIMUM_GROWTH: usize = 64 << 10;

/// The value of a parked row: the most a row holds, which no run of
/// spends brings below zero.
pub(crate) const PARKED: i64 = i64::MAX;

/// The table: a reservation of `CAPACITY_ROWS` rows and the sink after
/// them, committed as rows are armed, and how many rows are committed.
pub(crate) struct OptimizationBudgets {
    reservation: Reservation,
    /// How many rows are committed.
    rows: usize,
    /// Whether the sink's page is committed and the sink parked.
    sink_committed: bool,
    /// One bit per row, set while the row is armed. Rust's alone.
    armed: Vec<u64>,
    /// How far the next shrink goes (`shrink`).
    damping: super::limits::ShrinkDamping,
    /// Whether it committed more since the last shrink: demand, which
    /// keeps the next collection from shrinking it.
    grew: bool,
}

impl OptimizationBudgets {
    /// Reserves the table, committing none of it. A refusal is running out
    /// of memory, as the object space's is.
    pub(crate) fn new() -> OptimizationBudgets {
        let bytes = (CAPACITY_ROWS + 1) * ROW_BYTES;
        let reservation =
            Reservation::reserve(bytes).unwrap_or_else(|_| super::out_of_memory(bytes));
        OptimizationBudgets {
            reservation,
            rows: 0,
            sink_committed: false,
            armed: Vec::new(),
            damping: super::limits::ShrinkDamping::default(),
            grew: false,
        }
    }

    /// The address of row 0, fixed for the reservation's life.
    pub(crate) fn address(&self) -> u64 {
        self.reservation.base_address() as u64
    }

    /// How many rows are committed.
    #[cfg(test)]
    pub(crate) fn rows(&self) -> usize {
        self.rows
    }

    /// The address a copy of `row`'s baseline code spends from: the row's,
    /// its page committed, or the sink's for a code id past the capacity.
    pub(crate) fn spending_address(&mut self, row: CodeId) -> u64 {
        let index = row.index();
        if index >= CAPACITY_ROWS {
            self.commit_sink();
            return self.address() + (CAPACITY_ROWS * ROW_BYTES) as u64;
        }
        if index >= self.rows {
            self.commit_through(index);
        }
        self.address() + (index * ROW_BYTES) as u64
    }

    /// Whether `address` is the sink's.
    pub(crate) fn is_sink(&self, address: u64) -> bool {
        address == self.address() + (CAPACITY_ROWS * ROW_BYTES) as u64
    }

    /// Parks the sink again, should spends ever have brought it below zero.
    pub(crate) fn park_sink(&mut self) {
        self.commit_sink();
        let offset = CAPACITY_ROWS * ROW_BYTES;
        if let Some(word) = self.reservation.words_mut(offset..offset + ROW_BYTES) {
            word[0] = PARKED.cast_unsigned();
        }
    }

    /// Arms `row` with `budget`, committing the rows up to it first; a
    /// row past the capacity is not armed. Answers whether it was.
    pub(crate) fn arm(&mut self, row: CodeId, budget: i64) -> bool {
        let index = row.index();
        if index >= CAPACITY_ROWS {
            return false;
        }
        if index >= self.rows {
            self.commit_through(index);
        }
        self.write(index, budget);
        self.mark_armed(index, true);
        true
    }

    /// Raises an armed `row` to `at_least` when it holds less, so that a
    /// body whose feedback just changed waits for it to settle; a raise
    /// never lowers a budget, and leaves a row that is not armed alone.
    pub(crate) fn raise(&mut self, row: CodeId, at_least: i64) {
        let index = row.index();
        if self.is_armed(row) && self.read(index) < at_least {
            self.write(index, at_least);
        }
    }

    /// Parks an armed `row` at the most it holds: its body's optimization
    /// is requested, and its code spends on without calling the leaf.
    pub(crate) fn park(&mut self, row: CodeId) {
        if self.is_armed(row) {
            self.write(row.index(), PARKED);
        }
    }

    /// What `row` holds; 0 for a row not committed.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "read by tests until the optimizing compile reads a budget"
        )
    )]
    pub(crate) fn budget(&self, row: CodeId) -> i64 {
        self.read(row.index())
    }

    /// Whether `row` is armed: written by `arm` and not cleared since.
    pub(crate) fn is_armed(&self, row: CodeId) -> bool {
        let index = row.index();
        self.armed
            .get(index / 64)
            .is_some_and(|bits| bits & (1 << (index % 64)) != 0)
    }

    /// Clears `row`, if it is committed: 0, and not armed, so that the
    /// next spend from it calls the leaf, which arms it afresh.
    pub(crate) fn clear(&mut self, row: CodeId) {
        let index = row.index();
        if index < self.rows {
            self.write(index, 0);
            self.mark_armed(index, false);
        }
    }

    /// Arms every armed row afresh with `budget`.
    pub(crate) fn rearm_all(&mut self, budget: i64) {
        for row in 0..self.rows {
            if self
                .armed
                .get(row / 64)
                .is_some_and(|bits| bits & (1 << (row % 64)) != 0)
            {
                self.write(row, budget);
            }
        }
    }

    /// Clears every committed row.
    pub(crate) fn clear_all(&mut self) {
        if let Some(slots) = self.reservation.words_mut(0..self.rows * ROW_BYTES) {
            slots.fill(0);
        }
        self.armed.fill(0);
    }

    /// Gives back the committed pages above `high_rows` rows and `shrink`'s
    /// headroom once more than its threshold lies above them
    /// (`limits::Shrink`), and answers how many bytes that was.
    /// `high_rows` is the code table's length after a full collection
    /// truncated it: every row past it is cleared, the collection having
    /// cleared the row of every id it freed, and no machine code names
    /// one, every data word having been pointed at a row of the table's
    /// ids before.
    pub(crate) fn shrink(
        &mut self,
        high_rows: usize,
        shrink: super::limits::Shrink,
        damped: bool,
    ) -> usize {
        let committed = self.rows * ROW_BYTES;
        let Some(end) = shrink.keep(
            &mut self.damping,
            damped,
            std::mem::take(&mut self.grew),
            high_rows.min(self.rows) * ROW_BYTES,
            committed,
        ) else {
            return 0;
        };
        self.rows = end / ROW_BYTES;
        self.armed.truncate(self.rows.div_ceil(64));
        if let Some(bits) = self.armed.last_mut() {
            let kept = self.rows % 64;
            if kept != 0 {
                *bits &= (1 << kept) - 1;
            }
        }
        match self.reservation.decommit(end..committed) {
            Ok(()) => committed - end,
            // Counted uncommitted, the next growth commits them again.
            Err(_) => 0,
        }
    }

    fn read(&self, index: usize) -> i64 {
        if index >= self.rows {
            return 0;
        }
        let offset = index * ROW_BYTES;
        self.reservation
            .words(offset..offset + ROW_BYTES)
            .map_or(0, |word| word[0].cast_signed())
    }

    fn write(&mut self, index: usize, value: i64) {
        let offset = index * ROW_BYTES;
        if let Some(word) = self.reservation.words_mut(offset..offset + ROW_BYTES) {
            word[0] = value.cast_unsigned();
        }
    }

    fn mark_armed(&mut self, index: usize, armed: bool) {
        let word = index / 64;
        if armed && word >= self.armed.len() {
            self.armed.resize(word + 1, 0);
        }
        if let Some(bits) = self.armed.get_mut(word) {
            if armed {
                *bits |= 1 << (index % 64);
            } else {
                *bits &= !(1 << (index % 64));
            }
        }
    }

    /// Commits the sink's page and parks it, once.
    fn commit_sink(&mut self) {
        if self.sink_committed {
            return;
        }
        let page = Reservation::page_size();
        let start = CAPACITY_ROWS * ROW_BYTES;
        let end = (start + ROW_BYTES).div_ceil(page) * page;
        if self.reservation.commit(start..end).is_err() {
            super::out_of_memory(end - start);
        }
        self.sink_committed = true;
        self.park_sink();
    }

    /// Commits the rows through `row`, and a generous margin past it, which
    /// read as zeros -- not armed -- until a row is written.
    fn commit_through(&mut self, row: usize) {
        self.grew = true;
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
    use super::{OptimizationBudgets, CAPACITY_ROWS, PARKED};
    use crate::heap::CodeId;

    fn id(index: usize) -> CodeId {
        CodeId::from_index(index).expect("a small id")
    }

    #[test]
    fn a_raise_never_lowers_a_budget() {
        let mut budgets = OptimizationBudgets::new();
        budgets.arm(id(3), 50_000);
        budgets.raise(id(3), 20_000);
        assert_eq!(budgets.budget(id(3)), 50_000);
        budgets.raise(id(3), 70_000);
        assert_eq!(budgets.budget(id(3)), 70_000);
    }

    /// A row not armed -- never, or cleared since -- is left alone by a
    /// raise and a park, so that neither arms a row for a body whose code
    /// spends from it no more.
    #[test]
    fn only_an_armed_row_is_raised_or_parked() {
        let mut budgets = OptimizationBudgets::new();
        budgets.raise(id(5), 20_000);
        budgets.park(id(5));
        assert_eq!(budgets.budget(id(5)), 0);
        assert!(!budgets.is_armed(id(5)));
        budgets.arm(id(5), 10);
        budgets.park(id(5));
        assert_eq!(budgets.budget(id(5)), PARKED);
        budgets.clear(id(5));
        assert!(!budgets.is_armed(id(5)));
        assert_eq!(budgets.budget(id(5)), 0);
        budgets.arm(id(70), 1);
        budgets.clear_all();
        assert!(!budgets.is_armed(id(70)));
    }

    /// Each id spends from its own row, committed by the asking, and an id
    /// past the capacity from the parked sink.
    #[test]
    fn an_id_past_the_capacity_spends_from_the_parked_sink() {
        let mut budgets = OptimizationBudgets::new();
        let row = budgets.spending_address(id(9));
        assert_eq!(row, budgets.address() + 9 * 8);
        assert!(budgets.rows() > 9);
        let sink = budgets.spending_address(id(CAPACITY_ROWS));
        assert!(budgets.is_sink(sink));
        assert!(!budgets.is_sink(row));
        assert!(!budgets.arm(id(CAPACITY_ROWS), 5));
        assert_eq!(budgets.spending_address(id(CAPACITY_ROWS + 7)), sink);
    }
}
