//! Reserved, committed and protected memory for the object space.
//!
//! One `Reservation` is one contiguous address range, reserved once and
//! committed as a space grows, so that a handle can be a byte offset from a
//! fixed base (docs/design/memory.md, "The memory layer"). This is
//! the only crate in the workspace with `unsafe` code: every operating-system
//! call is in `platform`, and everything this crate hands out is safe to use.
//! The heap reads through `Reservation::prefix`, the committed read-write
//! range at the base, which is lent in O(1). The heap's young allocations
//! bump a cursor the reservation holds (`Reservation::bump`), one
//! comparison against a limit inside that prefix.
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(clippy::undocumented_unsafe_blocks)]

mod page_map;
mod platform;

use std::ops::Range;
use std::ptr::NonNull;

use page_map::PageMap;

/// How a committed page range may be accessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    /// Any access faults, and a stale offset into such a range finds no
    /// slice rather than reading whatever the collector left there. The
    /// heap protects nothing yet: its accessors lend only the words below
    /// the top, which already refuses an offset into the vacated tail
    /// (docs/design/memory.md, "The object space").
    NoAccess,
    /// Readable and writable.
    ReadWrite,
}

/// The operation a refusal came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Reserve,
    Commit,
    Decommit,
    Protect,
}

/// Why an operation on a reservation did not happen. Nothing here panics:
/// a refusal is a value, and what running out of memory does is the heap's
/// to decide -- today it aborts, as a failed allocation of the Rust
/// allocator does (docs/design/memory.md, "The object space").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    /// The range is empty, not aligned as the operation requires, or not
    /// inside the reservation. No system call was made.
    InvalidRange,
    /// `protect` on a range that is not entirely committed.
    NotCommitted,
    /// The operating system refused; `code` is its error number
    /// (`GetLastError` on Windows, `errno` elsewhere).
    Refused { operation: Operation, code: i32 },
}

impl std::fmt::Display for MemoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryError::InvalidRange => write!(
                formatter,
                "the range is not aligned or not inside the reservation"
            ),
            MemoryError::NotCommitted => write!(formatter, "the range is not entirely committed"),
            MemoryError::Refused { operation, code } => {
                let verb = match operation {
                    Operation::Reserve => "reserve",
                    Operation::Commit => "commit",
                    Operation::Decommit => "decommit",
                    Operation::Protect => "protect",
                };
                write!(
                    formatter,
                    "the operating system refused to {verb} memory (error {code})"
                )
            }
        }
    }
}

impl std::error::Error for MemoryError {}

/// Where the next bump allocation begins and where none may end, in byte
/// offsets. The limit never lies past the committed read-write prefix
/// (`set_bump` checks it, and `record` clamps it when the prefix
/// shrinks), and the top is a multiple of 8 inside the reservation, so a
/// bump that stays within the limit lends committed, initialized words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BumpCursor {
    top: usize,
    limit: usize,
}

/// One contiguous reserved address range and the record of which pages of it
/// are committed and how they are protected. The base never moves, so a byte
/// offset from it can serve as a handle. The heap's access path is
/// `prefix`: the end of the committed read-write range at the base is kept
/// as a field, refreshed whenever the page map changes.
pub struct Reservation {
    base: NonNull<u8>,
    size: usize,
    pages: PageMap,
    accessible_prefix: usize,
    bump: BumpCursor,
}

impl Reservation {
    /// The page size: commit, decommit and protect work on multiples of it.
    pub fn page_size() -> usize {
        platform::page_size()
    }

    /// Reserves at least `bytes` of address space, committing none of it.
    /// The size is rounded up to the platform's reservation granularity; a
    /// size whose rounding up overflows `usize` answers `InvalidRange`.
    pub fn reserve(bytes: usize) -> Result<Reservation, MemoryError> {
        if bytes == 0 {
            return Err(MemoryError::InvalidRange);
        }
        let granularity = platform::reservation_granularity();
        let size = bytes
            .checked_add(granularity - 1)
            .map(|rounded| rounded / granularity * granularity)
            .ok_or(MemoryError::InvalidRange)?;
        // SAFETY: reserving touches no memory the program owns.
        let base = unsafe { platform::reserve(size) }.map_err(|code| MemoryError::Refused {
            operation: Operation::Reserve,
            code,
        })?;
        Ok(Reservation {
            base,
            size,
            pages: PageMap::new(),
            accessible_prefix: 0,
            bump: BumpCursor { top: 0, limit: 0 },
        })
    }

    /// The reserved size in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Makes `range` committed and read-write. Pages committed before keep
    /// their contents; pages committed now read as zero.
    pub fn commit(&mut self, range: Range<usize>) -> Result<(), MemoryError> {
        self.check_pages(&range)?;
        // SAFETY: `check_pages` placed the range inside this reservation on
        // page boundaries.
        unsafe { platform::commit(self.address(range.start), range.len()) }.map_err(|code| {
            MemoryError::Refused {
                operation: Operation::Commit,
                code,
            }
        })?;
        self.record(range, Some(Protection::ReadWrite));
        Ok(())
    }

    /// Gives `range` back to the system; its contents are gone, and a later
    /// commit reads zeros. Takes `&mut self` so that no lent slice survives it.
    pub fn decommit(&mut self, range: Range<usize>) -> Result<(), MemoryError> {
        self.check_pages(&range)?;
        // SAFETY: the range is ours and page-aligned, and `&mut self` means no
        // slice into it is alive.
        let outcome = unsafe { platform::decommit(self.address(range.start), range.len()) };
        if let Err(code) = outcome {
            self.record_failure(range, None);
            return Err(MemoryError::Refused {
                operation: Operation::Decommit,
                code,
            });
        }
        self.record(range, None);
        Ok(())
    }

    /// Changes the protection of a committed `range`, keeping its contents.
    /// Takes `&mut self` so that no lent slice survives a change to no-access.
    pub fn protect(
        &mut self,
        range: Range<usize>,
        protection: Protection,
    ) -> Result<(), MemoryError> {
        self.check_pages(&range)?;
        if !self.pages.is_committed(range.clone()) {
            return Err(MemoryError::NotCommitted);
        }
        // SAFETY: the range is ours, page-aligned and committed, and `&mut self`
        // means no slice into it is alive.
        let outcome =
            unsafe { platform::protect(self.address(range.start), range.len(), protection) };
        if let Err(code) = outcome {
            if protection == Protection::NoAccess {
                self.record_failure(range, Some(Protection::NoAccess));
            }
            return Err(MemoryError::Refused {
                operation: Operation::Protect,
                code,
            });
        }
        self.record(range, Some(protection));
        Ok(())
    }

    /// The words of `range` -- byte offsets, 8-byte aligned -- when every
    /// byte of it is committed read-write, and `None` otherwise. An offset
    /// into a protected or uncommitted range finds no slice: that is the
    /// bounds check the object space's accessors rely on.
    pub fn words(&self, range: Range<usize>) -> Option<&[u64]> {
        self.check_words(&range)?;
        // SAFETY: `check_words` found the range inside the reservation,
        // 8-byte aligned (the base is page-aligned) and entirely committed
        // read-write; `&self` keeps `protect` and `decommit` away while the
        // slice lives, and committed memory is initialized (zero or written).
        Some(unsafe {
            std::slice::from_raw_parts(self.address(range.start).cast::<u64>(), range.len() / 8)
        })
    }

    /// As `words`, mutably.
    pub fn words_mut(&mut self, range: Range<usize>) -> Option<&mut [u64]> {
        self.check_words(&range)?;
        // SAFETY: as for `words`, and `&mut self` makes the slice unique.
        Some(unsafe {
            std::slice::from_raw_parts_mut(self.address(range.start).cast::<u64>(), range.len() / 8)
        })
    }

    /// Several disjoint ranges lent mutably at once -- the heap's spaces for
    /// one step -- or `None` when any is inaccessible or two overlap.
    pub fn split_words_mut<const COUNT: usize>(
        &mut self,
        ranges: [Range<usize>; COUNT],
    ) -> Option<[&mut [u64]; COUNT]> {
        for (index, range) in ranges.iter().enumerate() {
            self.check_words(range)?;
            for other in &ranges[index + 1..] {
                if range.start < other.end && other.start < range.end {
                    return None;
                }
            }
        }
        let base = self.base;
        Some(ranges.map(|range| {
            // SAFETY: every range was checked as `words_mut` checks one, and
            // no two overlap, so the slices never alias; `&mut self` keeps
            // everything else away while they live.
            unsafe {
                std::slice::from_raw_parts_mut(
                    base.as_ptr().wrapping_add(range.start).cast::<u64>(),
                    range.len() / 8,
                )
            }
        }))
    }

    /// A non-empty range inside the reservation, 8-byte aligned, committed
    /// read-write throughout.
    fn check_words(&self, range: &Range<usize>) -> Option<()> {
        let aligned = range.start.is_multiple_of(8) && range.end.is_multiple_of(8);
        let inside = range.start < range.end && range.end <= self.size;
        // `from_raw_parts` requires a byte length of at most `isize::MAX`,
        // which a reservation on a 32-bit target can exceed.
        let lendable = range.len() <= isize::MAX as usize;
        (aligned && lendable && inside && self.pages.is_accessible(range.clone())).then_some(())
    }

    /// Records the state of `range` after a system call on it failed. The rule:
    /// after a failure the map may under-report access but never over-report
    /// it. A failed `mprotect` may have changed some pages before it failed,
    /// and a failed `mmap(MAP_FIXED)` may already have discarded the old
    /// mapping, so the caller passes the most restrictive state the call
    /// could have left, and `words` never lends memory the map calls
    /// accessible but the system no longer does.
    fn record_failure(&mut self, range: Range<usize>, conservative: Option<Protection>) {
        self.record(range, conservative);
    }

    /// Every change to the page map goes through here, so the prefix the heap
    /// reads on every access is never stale.
    fn record(&mut self, range: Range<usize>, state: Option<Protection>) {
        self.pages.set(range, state);
        self.accessible_prefix = self.pages.accessible_prefix();
        // A shrunken prefix takes the cursor's limit with it, so no bump
        // lends memory that is no longer committed.
        self.bump.limit = self.bump.limit.min(self.accessible_prefix);
    }

    /// Where the next bump begins.
    #[inline(always)]
    pub fn bump_top(&self) -> usize {
        self.bump.top
    }

    /// Where no bump may end past.
    pub fn bump_limit(&self) -> usize {
        self.bump.limit
    }

    /// Places the bump cursor. Both offsets are multiples of 8; `top` lies
    /// inside the reservation and `limit` inside the committed read-write
    /// prefix. A `limit` below `top` is allowed, and refuses every bump
    /// until the cursor is placed again. `InvalidRange` otherwise, the
    /// cursor untouched.
    pub fn set_bump(&mut self, top: usize, limit: usize) -> Result<(), MemoryError> {
        let aligned = top.is_multiple_of(8) && limit.is_multiple_of(8);
        if !aligned || top > self.size || limit > self.accessible_prefix {
            return Err(MemoryError::InvalidRange);
        }
        self.bump = BumpCursor { top, limit };
        Ok(())
    }

    /// Bumps the cursor by `words` words when they end within its limit,
    /// and lends exactly those words, whatever they hold, with their byte
    /// offset; `None`, the cursor untouched, otherwise. One comparison:
    /// a `u8` count cannot carry the top past the end of the address
    /// space, and the subtraction saturates when the limit lies below the
    /// top.
    #[inline(always)]
    pub fn bump(&mut self, words: u8) -> Option<(usize, &mut [u64])> {
        let start = self.bump.top;
        let bytes = usize::from(words) * 8;
        if bytes > self.bump.limit.saturating_sub(start) {
            return None;
        }
        self.bump.top = start + bytes;
        // SAFETY: `start + bytes <= limit <= accessible_prefix`, so every
        // byte lent is committed read-write and initialized; `start` is a
        // multiple of 8 above a page-aligned base, so the words are
        // aligned; the cursor moved past them, so no later bump lends them
        // again, and `&mut self` keeps every other slice away while this
        // one lives.
        let words = unsafe {
            std::slice::from_raw_parts_mut(self.address(start).cast::<u64>(), usize::from(words))
        };
        Some((start, words))
    }

    /// `bump` for a fixed number of words, lent as an array, so that a
    /// caller's stores into it carry no index check.
    #[inline(always)]
    pub fn bump_array<const COUNT: usize>(&mut self) -> Option<(usize, &mut [u64; COUNT])> {
        const { assert!(COUNT <= u8::MAX as usize) };
        let start = self.bump.top;
        let bytes = COUNT * 8;
        if bytes > self.bump.limit.saturating_sub(start) {
            return None;
        }
        self.bump.top = start + bytes;
        // SAFETY: as for `bump`, for `COUNT` words; an array of `u64` has
        // their alignment.
        let words = unsafe { &mut *self.address(start).cast::<[u64; COUNT]>() };
        Some((start, words))
    }

    /// The cold bump, of any size, within `ceiling` rather than the limit:
    /// answers the byte offset of the first word, and lends nothing (the
    /// caller writes through `prefix_mut`). `None`, the cursor untouched,
    /// when the words would end past `ceiling` or `ceiling` lies past the
    /// committed read-write prefix.
    pub fn bump_up_to(&mut self, words: usize, ceiling: usize) -> Option<usize> {
        let start = self.bump.top;
        let end = words
            .checked_mul(8)
            .and_then(|bytes| start.checked_add(bytes))?;
        if end > ceiling || ceiling > self.accessible_prefix {
            return None;
        }
        self.bump.top = end;
        Some(start)
    }

    /// The words of `0..end` when every byte of it is committed read-write --
    /// the heap's space, read on every access, so this is one comparison
    /// against the end the page map last reported rather than a walk of it.
    /// `end` must be a multiple of 8; 0 lends an empty slice.
    ///
    /// Inlined, with `prefix_mut` and `check_prefix`: every object access of
    /// the heap goes through one of them, and a call there would outweigh the
    /// comparison it makes.
    #[inline]
    pub fn prefix(&self, end: usize) -> Option<&[u64]> {
        self.check_prefix(end)?;
        // SAFETY: `check_prefix` found `0..end` inside the committed read-write
        // prefix, which starts at the page-aligned base, so it is 8-byte
        // aligned and initialized; `&self` keeps `protect` and `decommit` away
        // while the slice lives.
        Some(unsafe { std::slice::from_raw_parts(self.base.as_ptr().cast::<u64>(), end / 8) })
    }

    /// As `prefix`, mutably.
    #[inline]
    pub fn prefix_mut(&mut self, end: usize) -> Option<&mut [u64]> {
        self.check_prefix(end)?;
        // SAFETY: as for `prefix`, and `&mut self` makes the slice unique.
        Some(unsafe { std::slice::from_raw_parts_mut(self.base.as_ptr().cast::<u64>(), end / 8) })
    }

    #[inline]
    fn check_prefix(&self, end: usize) -> Option<()> {
        (end.is_multiple_of(8) && end <= self.accessible_prefix && end <= isize::MAX as usize)
            .then_some(())
    }

    /// A non-empty range inside the reservation, on page boundaries.
    fn check_pages(&self, range: &Range<usize>) -> Result<(), MemoryError> {
        let page = platform::page_size();
        let aligned = range.start.is_multiple_of(page) && range.end.is_multiple_of(page);
        if range.start < range.end && range.end <= self.size && aligned {
            Ok(())
        } else {
            Err(MemoryError::InvalidRange)
        }
    }

    fn address(&self, offset: usize) -> *mut u8 {
        // `wrapping_add` keeps this safe; every caller has checked that
        // `offset` lies inside the reservation.
        self.base.as_ptr().wrapping_add(offset)
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        // SAFETY: `base` is this reservation's own base, and `&mut self` in
        // `drop` means nothing refers into it any more.
        unsafe { platform::release(self.base.as_ptr(), self.size) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prefix_is_lent_up_to_the_end_of_the_committed_read_write_range() {
        let page = Reservation::page_size();
        let mut reservation = Reservation::reserve(4 * page).expect("reserve");
        assert!(reservation.prefix(8).is_none(), "nothing committed yet");
        assert_eq!(reservation.prefix(0).map(<[u64]>::len), Some(0));
        reservation.commit(0..2 * page).expect("commit");
        assert_eq!(
            reservation.prefix(2 * page).map(<[u64]>::len),
            Some(2 * page / 8)
        );
        assert!(reservation.prefix(2 * page + 8).is_none());
        assert!(
            reservation.prefix(12).is_none(),
            "an end that is no word boundary"
        );
        reservation.commit(2 * page..3 * page).expect("commit");
        assert!(reservation.prefix(3 * page).is_some());
        reservation
            .protect(page..2 * page, Protection::NoAccess)
            .expect("protect");
        assert!(reservation.prefix(page).is_some());
        assert!(reservation.prefix(page + 8).is_none());
        reservation.decommit(0..page).expect("decommit");
        assert!(reservation.prefix(8).is_none());
    }

    #[test]
    fn a_write_through_the_mutable_prefix_reads_back_through_words() {
        let page = Reservation::page_size();
        let mut reservation = Reservation::reserve(page).expect("reserve");
        reservation.commit(0..page).expect("commit");
        let prefix = reservation.prefix_mut(16).expect("committed");
        prefix[1] = 0xABCD;
        assert_eq!(reservation.words(8..16), Some(&[0xABCD][..]));
    }

    #[test]
    fn a_recorded_failure_ends_the_prefix() {
        let page = Reservation::page_size();
        let mut reservation = Reservation::reserve(2 * page).expect("reserve");
        reservation.commit(0..2 * page).expect("commit");
        reservation.record_failure(page..2 * page, None);
        assert!(reservation.prefix(page).is_some());
        assert!(reservation.prefix(page + 8).is_none());
    }

    /// Pins the rule only: a range recorded after a failed call is no longer
    /// lent. It does not provoke an operating-system failure.
    #[test]
    fn a_range_recorded_after_a_failure_is_not_lent() {
        let page = Reservation::page_size();
        let mut reservation = Reservation::reserve(4 * page).expect("reserve");
        reservation.commit(0..2 * page).expect("commit");
        reservation.record_failure(0..page, Some(Protection::NoAccess));
        assert!(reservation.words(0..8).is_none());
        assert!(reservation.words(page..page + 8).is_some());
        reservation.record_failure(page..2 * page, None);
        assert!(reservation.words(page..page + 8).is_none());
    }

    /// A reservation of four pages with the first two committed.
    fn two_committed_pages() -> (Reservation, usize) {
        let page = Reservation::page_size();
        let mut reservation = Reservation::reserve(4 * page).expect("reserve");
        reservation.commit(0..2 * page).expect("commit");
        (reservation, page)
    }

    #[test]
    fn a_bump_lends_exactly_its_words_and_moves_the_top() {
        let (mut reservation, page) = two_committed_pages();
        reservation.set_bump(64, page).expect("inside the prefix");
        let (start, words) = reservation.bump(3).expect("room");
        assert_eq!(start, 64);
        assert_eq!(words.len(), 3);
        words.copy_from_slice(&[1, 2, 3]);
        assert_eq!(reservation.bump_top(), 64 + 3 * 8);
        let prefix = reservation.prefix(page).expect("committed");
        assert_eq!(&prefix[8..11], &[1, 2, 3]);
    }

    #[test]
    fn a_bump_past_the_limit_fails_and_leaves_the_top() {
        let (mut reservation, _) = two_committed_pages();
        reservation
            .set_bump(64, 64 + 2 * 8)
            .expect("inside the prefix");
        assert!(reservation.bump(3).is_none());
        assert_eq!(reservation.bump_top(), 64);
        assert!(reservation.bump(2).is_some());
        assert!(reservation.bump(1).is_none());
    }

    #[test]
    fn a_fixed_bump_lends_an_array() {
        let (mut reservation, page) = two_committed_pages();
        reservation.set_bump(0, page).expect("inside the prefix");
        let (start, words) = reservation.bump_array::<3>().expect("room");
        assert_eq!(start, 0);
        *words = [7, 8, 9];
        assert_eq!(reservation.bump_top(), 24);
    }

    #[test]
    fn set_bump_refuses_a_limit_past_the_committed_prefix_or_unaligned() {
        let (mut reservation, page) = two_committed_pages();
        assert_eq!(
            reservation.set_bump(0, 2 * page + 8),
            Err(MemoryError::InvalidRange)
        );
        assert_eq!(
            reservation.set_bump(4, page),
            Err(MemoryError::InvalidRange)
        );
        assert_eq!(
            reservation.set_bump(0, page + 4),
            Err(MemoryError::InvalidRange)
        );
        assert_eq!(
            reservation.set_bump(reservation.size() + 8, page),
            Err(MemoryError::InvalidRange)
        );
    }

    #[test]
    fn a_limit_below_the_top_refuses_every_bump() {
        let (mut reservation, page) = two_committed_pages();
        reservation
            .set_bump(page, 64)
            .expect("a limit below the top is allowed");
        assert!(reservation.bump(1).is_none());
        assert!(reservation.bump_array::<1>().is_none());
    }

    #[test]
    fn a_decommit_clamps_the_limit_to_the_new_prefix() {
        let (mut reservation, page) = two_committed_pages();
        reservation
            .set_bump(page - 16, 2 * page)
            .expect("inside the prefix");
        reservation.decommit(page..2 * page).expect("decommit");
        assert_eq!(reservation.bump_limit(), page);
        assert!(reservation.bump(2).is_some());
        assert!(reservation.bump(1).is_none());
    }

    #[test]
    fn the_cold_bump_takes_any_size_up_to_a_ceiling_inside_the_prefix() {
        let (mut reservation, page) = two_committed_pages();
        reservation.set_bump(0, 0).expect("inside the prefix");
        assert_eq!(reservation.bump_up_to(300, 2 * page), Some(0));
        assert_eq!(reservation.bump_top(), 300 * 8);
        assert_eq!(reservation.bump_up_to(1, 2 * page + 8), None);
        assert_eq!(reservation.bump_up_to(usize::MAX, 2 * page), None);
        assert_eq!(reservation.bump_top(), 300 * 8);
    }
}
