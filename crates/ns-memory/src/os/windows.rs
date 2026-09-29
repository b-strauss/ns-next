//! `VirtualAlloc` reserves and commits, `VirtualFree` decommits and releases,
//! `VirtualProtect` protects -- Strongtalk's `ReservedSpace`/`VirtualSpace`
//! shape (prior-art/strongtalk/vm/runtime/os_nt.cpp:403-414). Windows accounts
//! every commit, so a commit the system cannot back fails here as a value.

use std::ffi::c_void;
use std::ptr::NonNull;

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Memory::{
    VirtualAlloc, VirtualFree, VirtualProtect, MEM_COMMIT, MEM_DECOMMIT, MEM_RELEASE, MEM_RESERVE,
    PAGE_NOACCESS, PAGE_PROTECTION_FLAGS, PAGE_READWRITE,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

use crate::Protection;

fn system_info() -> SYSTEM_INFO {
    // SAFETY: `SYSTEM_INFO` is plain data, for which all-zero is a valid value.
    let mut information: SYSTEM_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: `GetSystemInfo` writes the structure it is given and nothing else.
    unsafe { GetSystemInfo(&mut information) };
    information
}

pub(crate) fn page_size() -> usize {
    system_info().dwPageSize as usize
}

/// A reservation's base and size are multiples of this, 64 KiB on every
/// current Windows, larger than a page.
pub(crate) fn reservation_granularity() -> usize {
    system_info().dwAllocationGranularity as usize
}

fn last_error() -> i32 {
    // SAFETY: `GetLastError` reads the calling thread's last-error value.
    unsafe { GetLastError() as i32 }
}

/// # Safety
/// None beyond the call itself; the answer is a fresh reservation no one else holds.
pub(crate) unsafe fn reserve(bytes: usize) -> Result<NonNull<u8>, i32> {
    // SAFETY: reserving with a null address asks the system to pick one; it
    // touches no memory the program owns.
    let address = unsafe { VirtualAlloc(std::ptr::null(), bytes, MEM_RESERVE, PAGE_NOACCESS) };
    NonNull::new(address.cast::<u8>()).ok_or_else(last_error)
}

/// # Safety
/// `address..address + bytes` lies inside one reservation this crate made and
/// is page-aligned.
pub(crate) unsafe fn commit(address: *mut u8, bytes: usize) -> Result<(), i32> {
    // SAFETY: the caller guarantees the range is inside our reservation;
    // committing already-committed pages keeps their contents.
    let answer =
        unsafe { VirtualAlloc(address.cast::<c_void>(), bytes, MEM_COMMIT, PAGE_READWRITE) };
    if answer.is_null() {
        Err(last_error())
    } else {
        Ok(())
    }
}

/// # Safety
/// As for `commit`, and no reference into the range is alive.
pub(crate) unsafe fn decommit(address: *mut u8, bytes: usize) -> Result<(), i32> {
    // SAFETY: the caller guarantees the range is ours and unborrowed.
    if unsafe { VirtualFree(address.cast::<c_void>(), bytes, MEM_DECOMMIT) } == 0 {
        Err(last_error())
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
        Protection::NoAccess => PAGE_NOACCESS,
        Protection::ReadWrite => PAGE_READWRITE,
    };
    let mut previous: PAGE_PROTECTION_FLAGS = 0;
    // SAFETY: the caller guarantees the range is ours, committed and unborrowed.
    if unsafe { VirtualProtect(address.cast::<c_void>(), bytes, flags, &mut previous) } == 0 {
        Err(last_error())
    } else {
        Ok(())
    }
}

/// # Safety
/// `address` is the base of a reservation this crate made, and nothing refers into it.
pub(crate) unsafe fn release(address: *mut u8, _bytes: usize) {
    // SAFETY: the caller guarantees this is our reservation's base and that it
    // is no longer used; `MEM_RELEASE` requires a size of zero.
    unsafe { VirtualFree(address.cast::<c_void>(), 0, MEM_RELEASE) };
}
