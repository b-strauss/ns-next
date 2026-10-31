//! The growth limits of the tables beside the object space
//! (`docs/design/memory.md`, "The trigger and the growth policy").
//!
//! The old space's limit asks for a full collection once the objects
//! grow; a program that edits methods or runs fresh families without
//! promoting much grows the tables beside the space instead -- code ids,
//! mixin and application rows, inline-cache sites -- which only a full
//! collection reclaims. Each has a limit of the collection limit's
//! shape: reached, it requests a full collection, and after every one it
//! becomes twice what the table still holds, never less than its floor.

/// A table's floor and its next limit, in rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GrowthLimit {
    /// The smallest limit a collection sets: a program whose table stays
    /// under it never collects for the table.
    pub(crate) floor: usize,
    /// The size at which the next mint requests a full collection.
    pub(crate) limit: usize,
}

impl GrowthLimit {
    /// A limit at `floor`, as before any collection.
    pub(crate) const fn at_floor(floor: usize) -> GrowthLimit {
        GrowthLimit {
            floor,
            limit: floor,
        }
    }

    /// Whether a table of `size` rows has reached the limit.
    #[inline]
    pub(crate) fn reached(&self, size: usize) -> bool {
        size >= self.limit
    }

    /// After a collection that left the table at `size_after_collection`
    /// rows: no more is minted until the next one than is still held, so
    /// the work a collection spends per minted row is amortized constant,
    /// as the old space's limit makes it per allocated byte.
    pub(crate) fn reset(&mut self, size_after_collection: usize) {
        self.limit = self.floor.max(size_after_collection.saturating_mul(2));
    }

    /// Sets the floor and the next limit to `floor` (`TableFloors`).
    pub(crate) fn set_floor(&mut self, floor: usize) {
        *self = GrowthLimit::at_floor(floor);
    }
}

/// The floor of the code ids (`Tables::codes`).
pub(crate) const CODE_ID_FLOOR: usize = 16_384;
/// The floor of the mixin rows (`Tables::mixins`).
pub(crate) const MIXIN_ROW_FLOOR: usize = 4_096;
/// The floor of the application rows (`Tables::applications`).
pub(crate) const APPLICATION_ROW_FLOOR: usize = 4_096;
/// The floor of the inline-cache sites (`InlineCaches::sites`): 8 MiB of
/// cells at 32 bytes a cell.
pub(crate) const SITE_FLOOR: usize = 262_144;
/// The floor of the program-made symbols' live entries.
pub(crate) const PROGRAM_SYMBOL_FLOOR: usize = 65_536;

/// The limits of the heap's own tables. The sites' limit lies with the
/// sites, in `InlineCaches`, which has no heap: it answers its own growth
/// past it (`InlineCaches::wants_collection`) and the mutator requests the
/// collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TableLimits {
    pub(crate) code_ids: GrowthLimit,
    pub(crate) mixin_rows: GrowthLimit,
    pub(crate) application_rows: GrowthLimit,
    pub(crate) program_symbols: GrowthLimit,
}

impl Default for TableLimits {
    fn default() -> TableLimits {
        TableLimits {
            code_ids: GrowthLimit::at_floor(CODE_ID_FLOOR),
            mixin_rows: GrowthLimit::at_floor(MIXIN_ROW_FLOOR),
            application_rows: GrowthLimit::at_floor(APPLICATION_ROW_FLOOR),
            program_symbols: GrowthLimit::at_floor(PROGRAM_SYMBOL_FLOOR),
        }
    }
}

/// When a reservation gives committed memory back, after a full
/// collection (`docs/design/memory.md`, "Giving committed memory back"):
/// once more than `threshold` bytes lie committed above what it still
/// uses -- its high mark -- everything above the high mark plus
/// `headroom` is decommitted. Spur's `attemptToShrink` after a full
/// collection, its `shrinkThreshold` against its `growHeadroom`
/// (`cointerp.c:50168, 50244-50249`). The headroom below the threshold is
/// the hysteresis: a high mark that rises and falls again within one
/// collection's cycle grows back into the headroom without a commit, and
/// a reservation shrunk once is not shrunk again until the high mark
/// falls by more than the threshold less the headroom.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shrink {
    /// The committed bytes above the high mark past which the reservation
    /// shrinks.
    pub threshold: usize,
    /// The bytes above the high mark a shrink keeps committed.
    pub headroom: usize,
}

impl Shrink {
    /// The committed end, in bytes, a reservation that uses `high_mark`
    /// bytes of its `committed` keeps: the high mark plus the headroom,
    /// rounded up to a page, when more than the threshold lies above the
    /// high mark and that end lies below what is committed; `None`
    /// otherwise.
    pub(crate) fn keep(&self, high_mark: usize, committed: usize) -> Option<usize> {
        let unused = committed.checked_sub(high_mark)?;
        if unused <= self.threshold {
            return None;
        }
        let page = ns_memory::Reservation::page_size();
        let end = high_mark
            .checked_add(self.headroom)?
            .checked_next_multiple_of(page)?;
        (end < committed).then_some(end)
    }
}

/// The activation stack's pair: a 100,000-deep recursion commits about
/// 16 MiB, which `heap::frames`'s growth reaches in steps of at most
/// 16 MiB.
pub(crate) const STACK_SHRINK: Shrink = Shrink {
    threshold: 4 << 20,
    headroom: 2 << 20,
};
/// The cells' pair: Spur's own defaults, 32 MiB and 16 MiB, the cells'
/// reservation being of the object space's size class (1 GiB).
pub(crate) const CELLS_SHRINK: Shrink = Shrink {
    threshold: 32 << 20,
    headroom: 16 << 20,
};
/// The machine entry table's pair, for 32 MiB of address space.
pub(crate) const MACHINE_ENTRIES_SHRINK: Shrink = Shrink {
    threshold: 1 << 20,
    headroom: 512 << 10,
};
/// The application id table's pair.
pub(crate) const APPLICATION_IDS_SHRINK: Shrink = Shrink {
    threshold: 1 << 20,
    headroom: 512 << 10,
};
/// The code zone's pair, for its 16 MiB by default.
pub(crate) const CODE_ZONE_SHRINK: Shrink = Shrink {
    threshold: 2 << 20,
    headroom: 1 << 20,
};

/// Every reservation's shrink pair at once, for a test that reaches a
/// threshold without committing up to the default first
/// (`Interpreter::set_shrink_pairs`).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShrinkPairs {
    pub stack: Shrink,
    pub cells: Shrink,
    pub machine_entries: Shrink,
    pub application_ids: Shrink,
    pub code_zone: Shrink,
}

impl Default for ShrinkPairs {
    fn default() -> ShrinkPairs {
        ShrinkPairs {
            stack: STACK_SHRINK,
            cells: CELLS_SHRINK,
            machine_entries: MACHINE_ENTRIES_SHRINK,
            application_ids: APPLICATION_IDS_SHRINK,
            code_zone: CODE_ZONE_SHRINK,
        }
    }
}

/// Every table's floor at once, for a test that reaches the limits
/// without minting up to the floors first
/// (`Interpreter::set_table_floors`).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableFloors {
    pub code_ids: usize,
    pub mixin_rows: usize,
    pub application_rows: usize,
    pub sites: usize,
    pub program_symbols: usize,
}

impl TableFloors {
    /// Every floor at `floor`.
    pub fn all(floor: usize) -> TableFloors {
        TableFloors {
            code_ids: floor,
            mixin_rows: floor,
            application_rows: floor,
            sites: floor,
            program_symbols: floor,
        }
    }
}

impl Default for TableFloors {
    fn default() -> TableFloors {
        TableFloors {
            code_ids: CODE_ID_FLOOR,
            mixin_rows: MIXIN_ROW_FLOOR,
            application_rows: APPLICATION_ROW_FLOOR,
            sites: SITE_FLOOR,
            program_symbols: PROGRAM_SYMBOL_FLOOR,
        }
    }
}

/// The size of every table that grows beside the object space, and of
/// what is committed for the reservations beside it, read without
/// disturbing them (`Interpreter::table_sizes`): what a long-run test
/// holds to a bound. A field a table not yet built would fill reads 0.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TableSizes {
    /// Rows of the code table, freed ones included.
    pub code_ids: usize,
    /// Rows of the mixin table, freed ones included.
    pub mixin_rows: usize,
    /// Rows of the application table, freed ones included.
    pub application_rows: usize,
    /// Inline-cache sites, free ranges included.
    pub sites: usize,
    /// Live symbols a program made.
    pub program_symbols: usize,
    /// Symbols this thread has interned.
    pub interned_symbols: usize,
    /// Bytes of the code zone installed code takes.
    pub zone_used_bytes: usize,
    /// Bodies installed in the code zone.
    pub zone_bodies: usize,
    /// Call return addresses recorded in the code zone.
    pub zone_returns: usize,
    /// Bytes of the activation stack committed.
    pub stack_committed_bytes: usize,
    /// Bytes of the activation stack below its top: its high mark.
    pub stack_used_bytes: usize,
    /// Bytes of the code zone committed.
    pub zone_committed_bytes: usize,
    /// Bytes of the cells' reservation committed.
    pub cells_committed_bytes: usize,
    /// Rows of the machine entry table committed.
    pub machine_entry_rows_committed: usize,
    /// Rows of the application id table committed.
    pub application_id_rows_committed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_limit_is_twice_what_a_collection_left_and_never_below_its_floor() {
        let mut limit = GrowthLimit::at_floor(64);
        assert!(!limit.reached(63));
        assert!(limit.reached(64));
        limit.reset(10);
        assert_eq!(limit.limit, 64);
        limit.reset(100);
        assert_eq!(limit.limit, 200);
        assert!(!limit.reached(199));
        limit.reset(usize::MAX);
        assert_eq!(limit.limit, usize::MAX);
    }

    /// A reservation shrinks only past its threshold, to its high mark
    /// plus the headroom on a page boundary, and never to an end at or
    /// above what it has committed.
    #[test]
    fn a_shrink_keeps_the_headroom_once_past_the_threshold() {
        let page = ns_memory::Reservation::page_size();
        let shrink = Shrink {
            threshold: 4 * page,
            headroom: 2 * page,
        };
        assert_eq!(shrink.keep(page, 5 * page), None, "at the threshold");
        assert_eq!(shrink.keep(page, 6 * page), Some(3 * page));
        assert_eq!(shrink.keep(page + 1, 6 * page), Some(4 * page));
        assert_eq!(shrink.keep(7 * page, 6 * page), None, "above the end");
        let wide = Shrink {
            threshold: page,
            headroom: 8 * page,
        };
        assert_eq!(wide.keep(0, 4 * page), None, "the headroom past the end");
    }
}
