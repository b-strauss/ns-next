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
/// a band, which says whether it shrinks and to what end, and a damping,
/// which says how much of the way it goes.
///
/// **The band.** Once more than `threshold` bytes lie committed above what
/// the reservation still uses -- its high mark -- and more than twice the
/// end it would keep, `round_up_to_page(high_mark + headroom)`, it
/// shrinks towards that end. Spur's `attemptToShrink` after a full
/// collection, its `shrinkThreshold` against its `growHeadroom`
/// (`cointerp.c:50168, 50244-50249`). Spur grows additively, by the larger
/// of what it needs and its headroom (`cointerp.c:26019`,
/// `:50328-50329`), and its pair is stable because the threshold is at
/// least the headroom plus one growth step. The reservations here grow by
/// doubling what they have committed (`heap::frames`, the cells, the
/// machine entry and application id tables), so their growth step from a
/// kept end is that end itself, and the second condition, `committed > 2
/// * end` -- a shrink keeps less than half of what is committed -- is
/// Spur's invariant restated for a doubling growth: one regrowth from a
/// kept end never puts the reservation back in the band. It is the band
/// HotSpot keeps between `MinHeapFreeRatio` and `MaxHeapFreeRatio`
/// (`prior-art/jdk/src/hotspot/share/gc/serial/tenuredGeneration.cpp:153-224`),
/// in bytes rather than ratios.
///
/// **The damping** (`ShrinkDamping`). A collection the band says shrink at
/// gives back only a factor's percent of what lies above the end, rounded
/// down to a page: 0% the first time, then 10%, 40% and 100%, and a
/// collection the band says no at sets the factor back to 0. HotSpot's
/// `ShrinkHeapInSteps` (`tenuredGeneration.cpp:145-151, 218-231`;
/// `prior-art/jdk/src/hotspot/share/memory/metaspace.cpp:425-427,
/// 503-522`), whose comment gives the reason: a program that collects
/// between phases would otherwise give back at once what its next phase
/// grows again. It counts collections, never time.
///
/// **Growth is demand.** A reservation that grew since the previous
/// collection -- a commit past its end, or for the activation stack a
/// push past the end its band would keep -- is out of the band at this
/// one: it does not shrink, and its damping starts again at 0. HotSpot
/// resets its factor wherever the generation does not want to shrink,
/// the factor read and zeroed before each resizing and the expanding
/// branch returning with it 0 (`tenuredGeneration.cpp:146-151, 181-193`);
/// a generation sees its demand in what is used after a collection. A
/// collection here runs at a low point -- the stack's top between two
/// recursions, a table truncated -- and does not see the peak between
/// two, so growth since the last one stands for it. The stack sees a
/// peak inside what it keeps committed too: a collection in its band
/// lowers its ordinary end to the band's end (`FrameStack::shrink`), and a
/// push past it is a growth, made in the cold path that raises the
/// ordinary end again, without a commit.
///
/// The guarantee: of the collections in a row that a reservation's band
/// says shrink at and that follow no growth, the first gives back
/// nothing, the second 10% of what lies above the band's end, the third
/// 40% of what is left above it, and the fourth and every later one all of
/// it; one collection out of the band, or after a growth, in between
/// starts the count again. So a program whose peak returns between every
/// two collections keeps what it committed, whatever the peak's depth,
/// and one that stops peaking gives it back over the ramp. After a shrink
/// kept `end` for a high mark `h`, a cycle whose peak lies within one
/// doubling of `end` never puts the reservation back in the band at a
/// collection whose high mark is `h` or above. The code zone commits only
/// what an install needs, so the band's second condition only makes its
/// shrink rarer.
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
    /// The band: the committed end, in bytes, a reservation that uses
    /// `high_mark` bytes of its `committed` shrinks towards -- the high
    /// mark plus the headroom, rounded up to a page -- when more than the
    /// threshold lies above the high mark and more than twice that end is
    /// committed; `None` otherwise.
    pub(crate) fn band(&self, high_mark: usize, committed: usize) -> Option<usize> {
        let unused = committed.checked_sub(high_mark)?;
        if unused <= self.threshold {
            return None;
        }
        let page = ns_memory::Reservation::page_size();
        let end = high_mark
            .checked_add(self.headroom)?
            .checked_next_multiple_of(page)?;
        (committed > end.checked_mul(2)?).then_some(end)
    }

    /// The committed end a reservation keeps when the band answers `end`
    /// and the damping's factor is `factor` percent: `factor` percent of
    /// what lies above the end given back, rounded down to a page; `None`
    /// when that is nothing. Multiplied before it is divided, unlike
    /// HotSpot (`tenuredGeneration.cpp:226`), whose division first guards
    /// a `size_t` against overflow: what lies above a reservation's end is
    /// at most a few GiB here, and the division first would leave up to 99
    /// bytes, and so a whole page, behind at 100%, which one doubling from
    /// that end would put back in the band.
    fn damped_end(end: usize, committed: usize, factor: usize) -> Option<usize> {
        let page = ns_memory::Reservation::page_size() as u64;
        // In 64 bits, where no product of an excess and a percent
        // overflows: a 32-bit target's would past about 41 MiB, and stop
        // short of the end at 100%.
        let excess = (committed - end) as u64;
        let given_back = excess * factor as u64 / 100 / page * page;
        let given_back = usize::try_from(given_back).unwrap_or(committed - end);
        (given_back > 0).then(|| committed - given_back)
    }

    /// What this collection decides for a reservation that uses
    /// `high_mark` bytes of its `committed`, by the band, the damping's
    /// factor, which this advances or resets (`ShrinkDamping::take`,
    /// `advance`), and whether it `grew` since the previous collection,
    /// which puts it out of the band. Undamped (`damped` false, the stress
    /// modes'), a collection in the band gives back all of the way at once,
    /// and the factor is left alone.
    pub(crate) fn decide(
        &self,
        damping: &mut ShrinkDamping,
        damped: bool,
        grew: bool,
        high_mark: usize,
        committed: usize,
    ) -> ShrinkDecision {
        let factor = damping.take(damped);
        let band_end = if grew {
            None
        } else {
            self.band(high_mark, committed)
        };
        let Some(end) = band_end else {
            return ShrinkDecision::default();
        };
        damping.advance(damped, factor);
        ShrinkDecision {
            band_end: Some(end),
            kept_end: Shrink::damped_end(end, committed, factor),
        }
    }

    /// The committed end `decide` keeps, `None` for no shrink.
    pub(crate) fn keep(
        &self,
        damping: &mut ShrinkDamping,
        damped: bool,
        grew: bool,
        high_mark: usize,
        committed: usize,
    ) -> Option<usize> {
        self.decide(damping, damped, grew, high_mark, committed)
            .kept_end
    }

    /// `keep` for a reservation of several regions, each with its high mark
    /// and committed bytes, which shrink together under one damping: the
    /// factor advances once if the band says shrink for any of them, and a
    /// growth of any puts them all out of it.
    pub(crate) fn keep_each<const COUNT: usize>(
        &self,
        damping: &mut ShrinkDamping,
        damped: bool,
        grew: bool,
        regions: [(usize, usize); COUNT],
    ) -> [Option<usize>; COUNT] {
        let factor = damping.take(damped);
        let ends = regions.map(|(high_mark, committed)| {
            if grew {
                None
            } else {
                self.band(high_mark, committed).map(|end| (end, committed))
            }
        });
        if ends.iter().any(Option::is_some) {
            damping.advance(damped, factor);
        }
        ends.map(|band| {
            band.and_then(|(end, committed)| Shrink::damped_end(end, committed, factor))
        })
    }
}

/// What a collection decides for one reservation (`Shrink::decide`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ShrinkDecision {
    /// The band's end, when the reservation is in its band: the activation
    /// stack lowers its ordinary end to it, so that a push past it shows
    /// the next collection a peak.
    pub(crate) band_end: Option<usize>,
    /// The committed end the reservation keeps, when it gives anything
    /// back.
    pub(crate) kept_end: Option<usize>,
}

/// A reservation's damping of its shrinks (`Shrink`): the percent of what
/// lies above the band's end the next collection that is in the band
/// gives back. HotSpot's `_shrink_factor`, read and reset at the start of
/// each collection's resizing and advanced only when it shrinks
/// (`tenuredGeneration.cpp:145-151, 226-231`; `metaspace.cpp:425-427,
/// 511-521`). Undamped, it is 100 throughout, as `_shrink_factor` is
/// without `ShrinkHeapInSteps` (`tenuredGeneration.cpp:294-295`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ShrinkDamping {
    factor: usize,
}

impl ShrinkDamping {
    /// The factor this collection uses, the state reset to 0 as HotSpot
    /// resets `_shrink_factor` before it decides: a collection out of the
    /// band leaves it at 0. Undamped, 100, the state untouched.
    fn take(&mut self, damped: bool) -> usize {
        if damped {
            std::mem::take(&mut self.factor)
        } else {
            100
        }
    }

    /// After a collection in the band that used `factor`: 10 after 0, else
    /// four times it, at most 100.
    fn advance(&mut self, damped: bool, factor: usize) {
        if damped {
            self.factor = if factor == 0 {
                10
            } else {
                (factor * 4).min(100)
            };
        }
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
/// The dispatching rows' pair (`heap/optimized_copies.rs`), for 48 MiB
/// of address space, the machine entry table's.
pub(crate) const OPTIMIZED_COPIES_SHRINK: Shrink = Shrink {
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
    pub optimized_copies: Shrink,
    pub application_ids: Shrink,
    pub code_zone: Shrink,
    /// Whether every shrink is damped (`ShrinkDamping`); false gives back
    /// all of the way at the first collection in the band, as the collect
    /// and zone stress modes do.
    pub damped: bool,
}

impl Default for ShrinkPairs {
    fn default() -> ShrinkPairs {
        ShrinkPairs {
            stack: STACK_SHRINK,
            cells: CELLS_SHRINK,
            machine_entries: MACHINE_ENTRIES_SHRINK,
            optimized_copies: OPTIMIZED_COPIES_SHRINK,
            application_ids: APPLICATION_IDS_SHRINK,
            code_zone: CODE_ZONE_SHRINK,
            damped: true,
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
    /// Bytes of the code zone its writer counts committed
    /// (`CodeWriter::committed`): pages it gave back are not counted, though
    /// a POSIX shared memory object outside Linux still holds them.
    pub zone_committed_bytes: usize,
    /// Bytes of the cells' reservation committed.
    pub cells_committed_bytes: usize,
    /// Rows of the machine entry table committed.
    pub machine_entry_rows_committed: usize,
    /// Rows of the application id table committed.
    pub application_id_rows_committed: usize,
    /// Bytes of the dispatching rows' reservation committed
    /// (`heap/optimized_copies.rs`), 0 before its first use.
    pub optimized_copies_committed_bytes: usize,
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

    /// The band: a reservation shrinks only past its threshold and past
    /// twice what it keeps, towards its high mark plus the headroom on a
    /// page boundary, so that one doubling from a kept end never puts it
    /// back in the band.
    #[test]
    fn the_band_is_past_the_threshold_and_twice_the_kept_end() {
        let page = ns_memory::Reservation::page_size();
        let shrink = Shrink {
            threshold: 4 * page,
            headroom: 2 * page,
        };
        assert_eq!(shrink.band(page, 5 * page), None, "at the threshold");
        assert_eq!(shrink.band(page, 7 * page), Some(3 * page));
        assert_eq!(shrink.band(page + 1, 9 * page), Some(4 * page));
        assert_eq!(shrink.band(7 * page, 6 * page), None, "above the end");
        // A shrink kept 4 pages for a high mark of one page and a byte;
        // one doubling commits 8, and a collection at the same high mark,
        // past the threshold by more than a page, is out of the band.
        assert_eq!(shrink.band(page + 1, 8 * page), None, "one regrowth");
        assert_eq!(
            shrink.band(0, 8 * page),
            Some(2 * page),
            "a lower high mark"
        );
        let wide = Shrink {
            threshold: page,
            headroom: 8 * page,
        };
        assert_eq!(wide.band(0, 4 * page), None, "the headroom past the end");
    }

    /// The damping: in the band, 0%, 10%, 40% and then all of what lies
    /// above the end, each rounded down to a page; one collection out of
    /// the band starts the ramp again; undamped, all of it at once and the
    /// ramp untouched.
    #[test]
    fn a_shrink_is_damped_zero_ten_forty_then_all() {
        let page = ns_memory::Reservation::page_size();
        let shrink = Shrink {
            threshold: page,
            headroom: page,
        };
        let mut damping = ShrinkDamping::default();
        let mut committed = 1001 * page;
        assert_eq!(shrink.keep(&mut damping, true, false, 0, committed), None);
        let excess = committed - page;
        committed -= excess * 10 / 100 / page * page;
        assert_eq!(
            shrink.keep(&mut damping, true, false, 0, 1001 * page),
            Some(committed)
        );
        let excess = committed - page;
        let after_forty = committed - excess * 40 / 100 / page * page;
        assert_eq!(
            shrink.keep(&mut damping, true, false, 0, committed),
            Some(after_forty)
        );
        assert_eq!(
            shrink.keep(&mut damping, true, false, 0, after_forty),
            Some(page)
        );
        // At 100 it stays; a collection out of the band resets it.
        assert_eq!(
            shrink.keep(&mut damping, true, false, 0, 1001 * page),
            Some(page)
        );
        assert_eq!(
            shrink.keep(&mut damping, true, false, 1000 * page, 1001 * page),
            None
        );
        assert_eq!(
            shrink.keep(&mut damping, true, false, 0, 1001 * page),
            None,
            "reset"
        );
        let mut undamped = ShrinkDamping::default();
        assert_eq!(
            shrink.keep(&mut undamped, false, false, 0, 1001 * page),
            Some(page)
        );
        assert_eq!(undamped, ShrinkDamping::default());
        // A growth since the previous collection puts it out of the band
        // and starts the ramp again: at 100% the growth gives back
        // nothing, nor does the collection after it.
        let mut grown = ShrinkDamping { factor: 100 };
        assert_eq!(shrink.keep(&mut grown, true, true, 0, 1001 * page), None);
        assert_eq!(shrink.keep(&mut grown, true, false, 0, 1001 * page), None);
        assert!(shrink
            .keep(&mut grown, true, false, 0, 1001 * page)
            .is_some());
        assert_eq!(
            shrink.keep(&mut undamped, false, true, 0, 1001 * page),
            None,
            "undamped too"
        );
    }
}
