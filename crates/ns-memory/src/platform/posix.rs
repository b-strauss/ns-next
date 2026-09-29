//! `mmap` with `PROT_NONE` reserves, `mprotect` commits and protects, a fixed
//! re-mapping with `PROT_NONE` decommits, `munmap` releases. The re-mapping
//! gives the pages back on Linux and macOS alike, where `madvise` with
//! `MADV_DONTNEED` does not on macOS; it is mapped without `MAP_NORESERVE`, so
//! a later commit is accounted where the kernel accounts commit
//! (docs/design/memory.md, "The memory layer").

use std::ptr::NonNull;

use crate::Protection;

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

pub(crate) fn page_size() -> usize {
    // SAFETY: `sysconf` reads a system constant.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(size).unwrap_or(4096)
}

pub(crate) fn reservation_granularity() -> usize {
    page_size()
}

/// # Safety
/// None beyond the call itself; the answer is a fresh mapping no one else holds.
pub(crate) unsafe fn reserve(bytes: usize) -> Result<NonNull<u8>, i32> {
    // SAFETY: a null hint lets the kernel choose the address; the mapping is
    // private, anonymous and inaccessible, so it touches nothing we own.
    let address = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            bytes,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    if address == libc::MAP_FAILED {
        Err(errno())
    } else {
        NonNull::new(address.cast::<u8>()).ok_or(0)
    }
}

/// # Safety
/// `address..address + bytes` lies inside one reservation this crate made and
/// is page-aligned.
pub(crate) unsafe fn commit(address: *mut u8, bytes: usize) -> Result<(), i32> {
    // SAFETY: the caller guarantees the range is ours; making committed pages
    // read-write again keeps their contents.
    if unsafe { libc::mprotect(address.cast(), bytes, libc::PROT_READ | libc::PROT_WRITE) } == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

/// # Safety
/// As for `commit`, and no reference into the range is alive.
pub(crate) unsafe fn decommit(address: *mut u8, bytes: usize) -> Result<(), i32> {
    // SAFETY: the caller guarantees the range is ours and unborrowed;
    // `MAP_FIXED` replaces exactly those pages with fresh inaccessible ones.
    let answer = unsafe {
        libc::mmap(
            address.cast(),
            bytes,
            libc::PROT_NONE,
            libc::MAP_FIXED | libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    if answer == libc::MAP_FAILED {
        Err(errno())
    } else {
        Ok(())
    }
}

/// # Safety
/// As for `decommit`, and the range is committed.
pub(crate) unsafe fn protect(
    address: *mut u8,
    bytes: usize,
    protection: Protection,
) -> Result<(), i32> {
    let flags = match protection {
        Protection::NoAccess => libc::PROT_NONE,
        Protection::ReadWrite => libc::PROT_READ | libc::PROT_WRITE,
    };
    // SAFETY: the caller guarantees the range is ours, committed and unborrowed.
    if unsafe { libc::mprotect(address.cast(), bytes, flags) } == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

/// # Safety
/// `address` is the base of a reservation of `bytes` this crate made, and nothing refers into it.
pub(crate) unsafe fn release(address: *mut u8, bytes: usize) {
    // SAFETY: the caller guarantees this is our whole reservation and unused.
    let answer = unsafe { libc::munmap(address.cast(), bytes) };
    debug_assert!(answer == 0, "munmap failed: {}", errno());
}
