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
}
