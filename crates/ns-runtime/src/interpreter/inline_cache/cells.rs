//! The cells (`docs/design/execution.md`, "Inline caches"): beside every
//! inline-cache entry, a copy machine code can read -- the guard to compare
//! a receiver's with, the address to call, and where the callee's
//! `current_class` is found -- in an `ns-memory` reservation of its own.
//!
//! The entries live in the `Mutator` (`InlineCaches`), whose `sites`, a
//! `Vec`, moves when it grows, and no word machine code reads between two
//! leaf calls may lie inside the mutator ("The machine words"). A cell is
//! at a fixed address instead: the reservation's base never moves, and
//! each region is committed as its table grows. A cell names no handle, so
//! nothing here is traced.
//!
//! A cell is written only from its entry, in the function that writes the
//! entry (`InlineCaches::advance` and its neighbours): the cache records
//! what the lookup decided, and the cell copies that record.
//!
//! Aliasing: machine code is to reach the cells through the reservation's
//! own pointer (`Reservation::base_address`), while Rust writes them
//! through slices each method here borrows afresh and drops before it
//! returns -- the argument `Reservation::base_address` makes for the
//! activation stack.

use ns_memory::Reservation;

use super::POLYMORPHIC_WIDTH;

/// One entry's cell: four words, at the address machine code computes
/// from the site's index (`Cells::site_address`).
///
/// `guard` is what a receiver's guard is compared with: the entry's
/// `DispatchId` for a public site and a non-private lexical one, and for a
/// `super` site and a private lexical one the word the key class's
/// `class_field::APPLICATION` holds (`Heap::class_application_word`) --
/// the application that determines the entry's guard -- so that machine
/// code compares a word of the class object it found. `entry` is the
/// address a hit calls (`jit::cell_entry`). `level` and `chain_start` say
/// where the callee's `current_class` is found: `level` superclass steps
/// from the class `chain_start` names. A polymorphic site's cell holds its
/// block's number in `level` instead.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub(crate) guard: u64,
    pub(crate) entry: u64,
    pub(crate) level: u64,
    pub(crate) chain_start: u64,
}

/// The class a cell's `level` is counted from (`Cell::chain_start`), as
/// `inline_cache::chain_start` finds it for a hit: the receiver's class
/// for a public or protected chain and its `doesNotUnderstand:` handler,
/// N for a private lexical one, S for a `super` one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChainStart {
    ReceiverClass = 0,
    LexicalClass = 1,
    Superclass = 2,
}

/// The guard of a cell that guards nothing -- a vacant, polymorphic or
/// megamorphic site, a vacant block slot, a vacant global row. Not
/// `DispatchId::VACANT`'s 1: that is the word a class object of
/// application 0 holds in its application field (a `SmallInteger` 0), so
/// a `super` or private lexical site would match it. No compared word is
/// all ones: a header's guard has 22 bits, and an application field holds
/// a `SmallInteger` -- the two bits above its tag clear -- or `nil`.
pub(crate) const VACANT_GUARD: u64 = u64::MAX;

impl Cell {
    /// What a cell reads while its entry is vacant.
    pub(crate) const VACANT: Cell = Cell {
        guard: VACANT_GUARD,
        entry: 0,
        level: 0,
        chain_start: ChainStart::ReceiverClass as u64,
    };

    pub(crate) const fn words(self) -> [u64; CELL_WORDS] {
        [self.guard, self.entry, self.level, self.chain_start]
    }

    /// The cell `words` -- `CELL_WORDS` of them -- hold.
    pub(crate) fn from_words(words: &[u64]) -> Cell {
        Cell {
            guard: words[0],
            entry: words[1],
            level: words[2],
            chain_start: words[3],
        }
    }
}

/// One row of the global cache's mirror, at the same group and way as its
/// `GlobalEntry`: the key -- the guard as the entry holds it, the
/// selector's index and the rule (`inline_cache::rule_word`) -- and the
/// rest of the cell of the entry's target. Eight words, the last two
/// padding, so that a row is one cache line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GlobalRow {
    pub(crate) guard: u64,
    pub(crate) selector: u64,
    pub(crate) rule: u64,
    pub(crate) entry: u64,
    pub(crate) level: u64,
    pub(crate) chain_start: u64,
}

impl GlobalRow {
    /// What a row reads while its entry is vacant.
    pub(crate) const VACANT: GlobalRow = GlobalRow {
        guard: VACANT_GUARD,
        selector: 0,
        rule: 0,
        entry: 0,
        level: 0,
        chain_start: ChainStart::ReceiverClass as u64,
    };

    pub(crate) const fn words(self) -> [u64; ROW_WORDS] {
        [
            self.guard,
            self.selector,
            self.rule,
            self.entry,
            self.level,
            self.chain_start,
            0,
            0,
        ]
    }

    /// The row `words` -- `ROW_WORDS` of them -- hold.
    pub(crate) fn from_words(words: &[u64]) -> GlobalRow {
        GlobalRow {
            guard: words[0],
            selector: words[1],
            rule: words[2],
            entry: words[3],
            level: words[4],
            chain_start: words[5],
        }
    }
}

pub(crate) const CELL_WORDS: usize = 4;
pub(crate) const CELL_BYTES: usize = CELL_WORDS * 8;
const BLOCK_BYTES: usize = CELL_BYTES * POLYMORPHIC_WIDTH;
pub(crate) const ROW_WORDS: usize = 8;
const ROW_BYTES: usize = ROW_WORDS * 8;

/// The global cache's mirror: room for 2^20 rows, far past the 1024 the
/// cache has unless a test asks for another size.
const GLOBAL_REGION_BYTES: usize = 64 << 20;
/// The site cells: room for 6,291,456 sites.
const SITE_REGION_BYTES: usize = 192 << 20;
/// The polymorphic blocks' cells: a block for every site, the most the
/// pool can hold, since a block is taken only by a site that names it and
/// a freed one is reused first.
const BLOCK_REGION_BYTES: usize = SITE_REGION_BYTES / CELL_BYTES * BLOCK_BYTES;
/// The whole reservation: 1 GiB of address space, committed as it grows.
const RESERVATION_BYTES: usize = GLOBAL_REGION_BYTES + SITE_REGION_BYTES + BLOCK_REGION_BYTES;

const _: () = assert!(RESERVATION_BYTES == 1 << 30);

/// The most sites the table may hold: one cell each.
pub(crate) const MAXIMUM_SITES: usize = SITE_REGION_BYTES / CELL_BYTES;
/// The most polymorphic blocks the pool may hold.
pub(crate) const MAXIMUM_BLOCKS: usize = BLOCK_REGION_BYTES / BLOCK_BYTES;
/// The most entries the global cache may have.
pub(crate) const MAXIMUM_GLOBAL_ROWS: usize = GLOBAL_REGION_BYTES / ROW_BYTES;

/// The least a region's growth commits, so that a small program commits
/// each region once.
pub(crate) const MINIMUM_GROWTH: usize = 64 << 10;

/// One table's part of the reservation: where it starts, how large it may
/// grow, and how much of it is committed from its start, in bytes.
struct Region {
    start: usize,
    size: usize,
    committed: usize,
}

impl Region {
    const fn new(start: usize, size: usize) -> Region {
        Region {
            start,
            size,
            committed: 0,
        }
    }
}

/// The cell reservation: the global cache's mirror, the site cells and the
/// polymorphic blocks' cells, each region committed as its table grows.
pub(crate) struct Cells {
    reservation: Reservation,
    global: Region,
    sites: Region,
    blocks: Region,
}

impl Cells {
    /// Reserves the 1 GiB, committing none of it. A refusal is running out
    /// of memory, as the object space's is.
    pub(crate) fn new() -> Cells {
        let reservation = Reservation::reserve(RESERVATION_BYTES)
            .unwrap_or_else(|_| crate::heap::out_of_memory(RESERVATION_BYTES));
        Cells {
            reservation,
            global: Region::new(0, GLOBAL_REGION_BYTES),
            sites: Region::new(GLOBAL_REGION_BYTES, SITE_REGION_BYTES),
            blocks: Region::new(GLOBAL_REGION_BYTES + SITE_REGION_BYTES, BLOCK_REGION_BYTES),
        }
    }

    /// The address of site `index`'s cell: the site cells' base plus
    /// `index` cells, fixed for the reservation's life.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "a compiled send reads its site's cell from it")
    )]
    pub(crate) fn site_address(&self, index: usize) -> usize {
        self.reservation.base_address() + self.sites.start + index * CELL_BYTES
    }

    /// The address of polymorphic block `block`'s four cells, in the order
    /// of its slots.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the polymorphic stub scans a block from it")
    )]
    pub(crate) fn block_address(&self, block: u32) -> usize {
        self.reservation.base_address() + self.blocks.start + block as usize * BLOCK_BYTES
    }

    /// The address of the global cache's mirror: row `index` lies `index`
    /// rows past it, so group `group`'s two ways are rows `2 * group` and
    /// `2 * group + 1`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the megamorphic stub probes the mirror from it")
    )]
    pub(crate) fn global_address(&self) -> usize {
        self.reservation.base_address() + self.global.start
    }

    /// Site `index`'s cell; `Cell::VACANT` for one never written.
    pub(crate) fn site(&self, index: usize) -> Cell {
        read_cell(&self.reservation, &self.sites, index * CELL_BYTES)
    }

    /// Writes site `index`'s cell.
    pub(crate) fn write_site(&mut self, index: usize, cell: Cell) {
        write_cells(
            &mut self.reservation,
            &mut self.sites,
            index * CELL_BYTES,
            &[cell],
        );
    }

    /// Writes every site cell of `range` vacant.
    pub(crate) fn vacate_sites(&mut self, range: std::ops::Range<usize>) {
        let cells = vec![Cell::VACANT; range.len()];
        write_cells(
            &mut self.reservation,
            &mut self.sites,
            range.start * CELL_BYTES,
            &cells,
        );
    }

    /// Writes the cell of slot `slot` of polymorphic block `block`.
    pub(crate) fn write_block_cell(&mut self, block: u32, slot: usize, cell: Cell) {
        write_cells(
            &mut self.reservation,
            &mut self.blocks,
            block as usize * BLOCK_BYTES + slot * CELL_BYTES,
            &[cell],
        );
    }

    /// Writes every cell of the blocks of `blocks` vacant.
    pub(crate) fn vacate_blocks(&mut self, blocks: std::ops::Range<usize>) {
        let cells = vec![Cell::VACANT; blocks.len() * POLYMORPHIC_WIDTH];
        write_cells(
            &mut self.reservation,
            &mut self.blocks,
            blocks.start * BLOCK_BYTES,
            &cells,
        );
    }

    /// Global row `index`; `GlobalRow::VACANT` for one never written.
    pub(crate) fn global_row(&self, index: usize) -> GlobalRow {
        let offset = index * ROW_BYTES;
        if offset + ROW_BYTES > self.global.committed {
            return GlobalRow::VACANT;
        }
        let start = self.global.start + offset;
        self.reservation
            .words(start..start + ROW_BYTES)
            .map_or(GlobalRow::VACANT, GlobalRow::from_words)
    }

    /// The words of the first `count` site cells, of the first `count`
    /// blocks' cells in slot order, and of the first `count` global rows,
    /// each one slice: what `InlineCaches::verify_cells` compares, which a
    /// stress mode runs before every scavenge. Empty where `count` is past
    /// what any write has committed, which a table never is.
    pub(crate) fn site_words(&self, count: usize) -> &[u64] {
        prefix_words(&self.reservation, &self.sites, count * CELL_BYTES)
    }

    /// See `site_words`.
    pub(crate) fn block_words(&self, count: usize) -> &[u64] {
        prefix_words(&self.reservation, &self.blocks, count * BLOCK_BYTES)
    }

    /// See `site_words`.
    pub(crate) fn global_words(&self, count: usize) -> &[u64] {
        prefix_words(&self.reservation, &self.global, count * ROW_BYTES)
    }

    /// Writes global row `index`.
    pub(crate) fn write_global_row(&mut self, index: usize, row: GlobalRow) {
        self.write_global_rows(index, &[row]);
    }

    /// Exchanges global rows `first` and `second`, as the global cache's
    /// promotion exchanges their entries.
    pub(crate) fn swap_global_rows(&mut self, first: usize, second: usize) {
        let (first_row, second_row) = (self.global_row(first), self.global_row(second));
        self.write_global_row(first, second_row);
        self.write_global_row(second, first_row);
    }

    /// Writes every global row of `rows` vacant.
    pub(crate) fn vacate_global_rows(&mut self, rows: std::ops::Range<usize>) {
        let vacant = vec![GlobalRow::VACANT; rows.len()];
        self.write_global_rows(rows.start, &vacant);
    }

    /// Writes `rows` from row `index` on, committing what they need first.
    fn write_global_rows(&mut self, index: usize, rows: &[GlobalRow]) {
        if rows.is_empty() {
            return;
        }
        let offset = index * ROW_BYTES;
        let end = offset + rows.len() * ROW_BYTES;
        if !ensure_committed(&mut self.reservation, &mut self.global, end) {
            return;
        }
        let range = self.global.start + offset..self.global.start + end;
        if let Some(words) = self.reservation.words_mut(range) {
            for (slot, row) in words.as_chunks_mut::<ROW_WORDS>().0.iter_mut().zip(rows) {
                *slot = row.words();
            }
        }
    }
}

/// The cell at `offset` bytes into `region`; `Cell::VACANT` past its
/// committed part, which no table has reached.
fn read_cell(reservation: &Reservation, region: &Region, offset: usize) -> Cell {
    if offset + CELL_BYTES > region.committed {
        return Cell::VACANT;
    }
    let start = region.start + offset;
    reservation
        .words(start..start + CELL_BYTES)
        .map_or(Cell::VACANT, Cell::from_words)
}

/// The words of the first `bytes` of `region`; empty when they are not
/// all committed.
fn prefix_words<'reservation>(
    reservation: &'reservation Reservation,
    region: &Region,
    bytes: usize,
) -> &'reservation [u64] {
    if bytes == 0 || bytes > region.committed {
        return &[];
    }
    reservation
        .words(region.start..region.start + bytes)
        .unwrap_or_default()
}

/// Writes `cells` from `offset` bytes into `region`, committing what they
/// need first.
fn write_cells(reservation: &mut Reservation, region: &mut Region, offset: usize, cells: &[Cell]) {
    if cells.is_empty() {
        return;
    }
    let end = offset + cells.len() * CELL_BYTES;
    if !ensure_committed(reservation, region, end) {
        return;
    }
    if let Some(words) = reservation.words_mut(region.start + offset..region.start + end) {
        for (slot, cell) in words.as_chunks_mut::<CELL_WORDS>().0.iter_mut().zip(cells) {
            *slot = cell.words();
        }
    }
}

/// Commits `region` up to at least `end` bytes from its start: double what
/// is committed, at least `MINIMUM_GROWTH`, within the region, and only
/// what `end` needs when the operating system refuses that much; a refusal
/// of that is running out of memory. False, committing nothing, past the
/// region's end, which no table reaches: `InlineCaches` grows none past
/// `MAXIMUM_SITES`, `MAXIMUM_BLOCKS` or `MAXIMUM_GLOBAL_ROWS`.
fn ensure_committed(reservation: &mut Reservation, region: &mut Region, end: usize) -> bool {
    if end <= region.committed {
        return true;
    }
    if end > region.size {
        debug_assert!(false, "internal: a cache table grew past its cells' region");
        return false;
    }
    let page = Reservation::page_size();
    let needed = end.div_ceil(page) * page;
    let generous = (region.committed + region.committed.max(MINIMUM_GROWTH))
        .div_ceil(page)
        .saturating_mul(page)
        .min(region.size);
    let target = needed.max(generous);
    let from = region.start + region.committed;
    region.committed = if reservation.commit(from..region.start + target).is_ok() {
        target
    } else if reservation.commit(from..region.start + needed).is_ok() {
        needed
    } else {
        crate::heap::out_of_memory(needed - region.committed)
    };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::inline_cache::{InlineCaches, Target};
    use crate::mixin::{AccessorKind, DispatchId};

    /// A site's cell is at the cell table's base plus its index, so its
    /// address holds while the site table -- a `Vec` that moves -- grows
    /// past several of the cells' commit steps, and the cell still reads
    /// what the fill wrote.
    #[test]
    fn a_site_cell_is_at_a_fixed_address_while_the_table_grows() {
        let mut caches = InlineCaches::new();
        let jit = crate::jit::JitState::new();
        let index = caches.fresh_range(1).index();
        let address = caches.cells.site_address(index);
        let guard = DispatchId::from_counter(DispatchId::FIRST_MINTED);
        let target = Target::Slot {
            field: 0,
            kind: AccessorKind::Getter,
            chain: crate::interpreter::resolve::Chain::Public,
            level: crate::heap::ApplicationIndex::UNREGISTERED,
            recorded_guard: DispatchId::NONE,
        };
        let written = Cell {
            guard: u64::from(guard.bits()),
            entry: crate::jit::cell_entry(&jit, &target),
            level: 0,
            chain_start: ChainStart::ReceiverClass as u64,
        };
        caches.advance(index, guard, target, false, written, &jit);
        let grown = MINIMUM_GROWTH / CELL_BYTES * 8;
        let base = caches.fresh_range(u32::try_from(grown).expect("a small range"));
        assert_eq!(caches.cells.site_address(index), address);
        assert_eq!(caches.cells.site(index), written);
        let last = base.index() + grown - 1;
        assert_eq!(caches.cells.site(last), Cell::VACANT);
        assert_eq!(
            caches.cells.site_address(last),
            address + (last - index) * CELL_BYTES
        );
    }

    /// The three tables lie where machine code computes them from the
    /// reservation: the global mirror at its base, the site cells after
    /// it, a block's four cells after the sites', and every region inside
    /// the 1 GiB.
    #[test]
    fn the_cells_lie_where_machine_code_computes_them() {
        let cells = Cells::new();
        let base = cells.global_address();
        assert_eq!(cells.site_address(0), base + GLOBAL_REGION_BYTES);
        assert_eq!(
            cells.block_address(0),
            base + GLOBAL_REGION_BYTES + SITE_REGION_BYTES
        );
        assert_eq!(cells.block_address(1) - cells.block_address(0), BLOCK_BYTES);
        assert_eq!(
            cells.block_address(u32::try_from(MAXIMUM_BLOCKS).expect("a u32")),
            base + RESERVATION_BYTES
        );
        assert_eq!(
            cells.site_address(MAXIMUM_SITES),
            cells.block_address(0),
            "the sites' region ends where the blocks' begins"
        );
        assert_eq!(std::mem::size_of::<Cell>(), CELL_BYTES);
    }

    /// A global row moves with its entry: the second way's key, read,
    /// is promoted to the first way, its row with it, and the row a third
    /// key's insert writes takes the first way while the displaced one
    /// moves to the second.
    #[test]
    fn a_global_row_moves_with_its_entry() {
        let mut caches = InlineCaches::new();
        caches.set_global_cache_size(2);
        let jit = crate::jit::JitState::new();
        let selector = crate::Symbol::NEW;
        let guard = |number: u32| DispatchId::from_counter(DispatchId::FIRST_MINTED + number);
        let slot = |field| Target::Slot {
            field,
            kind: AccessorKind::Getter,
            chain: crate::interpreter::resolve::Chain::Public,
            level: crate::heap::ApplicationIndex::UNREGISTERED,
            recorded_guard: DispatchId::NONE,
        };
        let rule = crate::interpreter::inline_cache::CacheRule::Public;
        for number in 0..2 {
            let target = slot(number);
            let cell =
                crate::interpreter::inline_cache::tests::public_cell(guard(number), &target, &jit);
            caches.global_insert(guard(number), selector, rule, target, cell);
        }
        let guard_word = |number: u32| u64::from(guard(number).bits());
        assert_eq!(caches.cells.global_row(0).guard, guard_word(1));
        assert_eq!(caches.cells.global_row(1).guard, guard_word(0));
        assert!(caches.global_get(guard(0), selector, rule).is_some());
        assert_eq!(caches.cells.global_row(0).guard, guard_word(0));
        assert_eq!(caches.cells.global_row(1).guard, guard_word(1));
        assert_eq!(caches.verify_cells_against(&|_| None, &jit), Ok(()));
        caches.invalidate();
        assert_eq!(caches.cells.global_row(0), GlobalRow::VACANT);
        assert_eq!(caches.verify_cells_against(&|_| None, &jit), Ok(()));
    }
}
