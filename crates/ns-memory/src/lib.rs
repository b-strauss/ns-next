//! Reserved, committed and protected memory for the object space.
//!
//! One `Reservation` is one contiguous address range, reserved once and
//! committed as a space grows, so that a handle can be a byte offset from a
//! fixed base (docs/design/memory.md, "Planned: the memory layer"). This is
//! the only crate in the workspace with `unsafe` code: every operating-system
//! call is in `platform`, and everything this crate hands out is safe to use.
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
    /// Any access faults. A debug build puts a vacated range here, so that a
    /// stale offset into it finds no slice rather than reading whatever the
    /// collector left there.
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
/// running out of memory is something the heap reports, not a crash
/// (docs/design/memory.md, "Planned: the memory layer").
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
                write!(
                    formatter,
                    "the operating system refused to {operation:?} memory (error {code})"
                )
            }
        }
    }
}

impl std::error::Error for MemoryError {}

/// One contiguous reserved address range and the record of which pages of it
/// are committed and how they are protected. The base never moves, so a byte
/// offset from it can serve as a handle.
pub struct Reservation {
    base: NonNull<u8>,
    size: usize,
    pages: PageMap,
}

impl Reservation {
    /// The page size: commit, decommit and protect work on multiples of it.
    pub fn page_size() -> usize {
        platform::page_size()
    }

    /// Reserves at least `bytes` of address space, committing none of it.
    /// The size is rounded up to the platform's reservation granularity.
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
        self.pages.set(range, Some(Protection::ReadWrite));
        Ok(())
    }

    /// Gives `range` back to the system; its contents are gone, and a later
    /// commit reads zeros. Takes `&mut self` so that no lent slice survives it.
    pub fn decommit(&mut self, range: Range<usize>) -> Result<(), MemoryError> {
        self.check_pages(&range)?;
        // SAFETY: the range is ours and page-aligned, and `&mut self` means no
        // slice into it is alive.
        unsafe { platform::decommit(self.address(range.start), range.len()) }.map_err(|code| {
            MemoryError::Refused {
                operation: Operation::Decommit,
                code,
            }
        })?;
        self.pages.set(range, None);
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
        unsafe { platform::protect(self.address(range.start), range.len(), protection) }.map_err(
            |code| MemoryError::Refused {
                operation: Operation::Protect,
                code,
            },
        )?;
        self.pages.set(range, Some(protection));
        Ok(())
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
