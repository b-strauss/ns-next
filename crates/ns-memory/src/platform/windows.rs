//! `VirtualAlloc` reserves and commits, `VirtualFree` decommits and releases,
//! `VirtualProtect` protects -- Strongtalk's `ReservedSpace`/`VirtualSpace`
//! shape (prior-art/strongtalk/vm/runtime/os_nt.cpp:403-414). Windows accounts
//! every commit, so a commit the system cannot back fails here as a value.
//!
//! The code zone is a pagefile-backed section mapped twice, one view
//! executable and one writable, so no page is ever writable and executable
//! at once (docs/design/execution.md, "Executable memory").

use std::ffi::c_void;
use std::ptr::NonNull;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, DiscardVirtualMemory, MapViewOfFile, UnmapViewOfFile, VirtualAlloc,
    VirtualFree, VirtualProtect, FILE_MAP, FILE_MAP_EXECUTE, FILE_MAP_READ, FILE_MAP_WRITE,
    MEMORY_MAPPED_VIEW_ADDRESS, MEM_COMMIT, MEM_DECOMMIT, MEM_RELEASE, MEM_RESERVE,
    PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_NOACCESS, PAGE_PROTECTION_FLAGS,
    PAGE_READWRITE, SEC_RESERVE,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::Threading::{GetCurrentThreadStackLimits, SetThreadStackGuarantee};

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
    let answer = unsafe { VirtualFree(address.cast::<c_void>(), 0, MEM_RELEASE) };
    debug_assert!(
        answer != 0,
        "VirtualFree(MEM_RELEASE) failed: {}",
        last_error()
    );
}

/// The lowest address of the running thread's stack a frame may use: the
/// stack's reserved lower limit (`GetCurrentThreadStackLimits`) raised by
/// what the system keeps below the last usable page -- the guard page,
/// the page under it, which is never committed, and the guarantee the
/// thread keeps for its stack-overflow handler (`SetThreadStackGuarantee`
/// asked with 0, which answers the guarantee and changes nothing).
pub(crate) fn thread_stack_lowest_usable_address() -> Option<usize> {
    let mut lowest = 0usize;
    let mut highest = 0usize;
    // SAFETY: `GetCurrentThreadStackLimits` writes the two words it is
    // given and nothing else.
    unsafe { GetCurrentThreadStackLimits(&mut lowest, &mut highest) };
    if lowest == 0 || highest <= lowest {
        return None;
    }
    let mut guarantee = 0u32;
    // An unknown guarantee is an unknown bound: no answer, which runs
    // interpreted, rather than one that may promise the guarantee's pages.
    // SAFETY: with a value of 0 the call writes the current guarantee into
    // the word it is given and changes nothing.
    if unsafe { SetThreadStackGuarantee(&mut guarantee) } == 0 {
        return None;
    }
    lowest
        .checked_add(2 * page_size())
        .and_then(|address| address.checked_add(guarantee as usize))
}

/// How the code zone's executable view may be accessed. Neither ever
/// includes write, which is what keeps a stray store from rewriting code
/// that runs.
const EXECUTABLE_VIEW_ACCESS: FILE_MAP = FILE_MAP_READ | FILE_MAP_EXECUTE;
const EXECUTABLE_VIEW_PROTECTION: PAGE_PROTECTION_FLAGS = PAGE_EXECUTE_READ;

/// Any committed page of the executable view's first `committed` bytes is
/// writable, as the system reports its protection now (`VirtualQuery`), so
/// that a commit which left the view writable is caught, not only a
/// mapping asked for with write.
#[cfg(test)]
pub(crate) fn executable_view_is_writable(mapping: &CodeMapping, committed: usize) -> bool {
    use windows_sys::Win32::System::Memory::{
        VirtualQuery, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_WRITECOPY, PAGE_WRITECOPY,
    };
    let writable_protections = [
        PAGE_READWRITE,
        PAGE_WRITECOPY,
        PAGE_EXECUTE_READWRITE,
        PAGE_EXECUTE_WRITECOPY,
    ];
    // The modifier bits (guard, no-cache, write-combine) sit above the base
    // protection's byte.
    let base_protection_mask: PAGE_PROTECTION_FLAGS = 0xFF;
    let mut offset = 0;
    while offset < committed {
        let address = mapping.executable.as_ptr().wrapping_add(offset);
        // SAFETY: `MEMORY_BASIC_INFORMATION` is plain data, for which all-zero
        // is a valid value.
        let mut information: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: querying an address inside our own view reads nothing the
        // program owns and writes only the structure it is given.
        let written = unsafe {
            VirtualQuery(
                address.cast::<c_void>(),
                &mut information,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        assert!(written != 0, "VirtualQuery failed: {}", last_error());
        if information.State == MEM_COMMIT
            && writable_protections.contains(&(information.Protect & base_protection_mask))
        {
            return true;
        }
        let region_end = information.BaseAddress as usize + information.RegionSize;
        offset = region_end - mapping.executable.as_ptr() as usize;
    }
    false
}

/// The two views of one code zone. The section itself needs no handle: the
/// views keep it alive, and unmapping both releases it.
pub(crate) struct CodeMapping {
    pub(crate) executable: NonNull<u8>,
    pub(crate) writable: NonNull<u8>,
}

/// # Safety
/// None beyond the call itself; the answer is two fresh views no one else
/// holds. `bytes` is a multiple of the reservation granularity.
pub(crate) unsafe fn map_code(bytes: usize) -> Result<CodeMapping, i32> {
    let size = u64::try_from(bytes).map_err(|_| 0)?;
    let high = u32::try_from(size >> 32).map_err(|_| 0)?;
    let low = u32::try_from(size & u64::from(u32::MAX)).map_err(|_| 0)?;
    // SAFETY: a pagefile-backed, unnamed section touches no memory the
    // program owns. `SEC_RESERVE` reserves its pages without committing
    // them, so a zone commits only what code fills; the section's maximum
    // protection must admit both views.
    let section = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_EXECUTE_READWRITE | SEC_RESERVE,
            high,
            low,
            std::ptr::null(),
        )
    };
    if section.is_null() {
        return Err(last_error());
    }
    let map = |access: FILE_MAP| {
        // SAFETY: `section` is the live section just created, `bytes` long.
        let view = unsafe { MapViewOfFile(section, access, 0, 0, bytes) };
        NonNull::new(view.Value.cast::<u8>()).ok_or_else(last_error)
    };
    let views = map(EXECUTABLE_VIEW_ACCESS).and_then(|executable| match map(FILE_MAP_WRITE) {
        Ok(writable) => Ok(CodeMapping {
            executable,
            writable,
        }),
        Err(code) => {
            // SAFETY: the view was mapped above and nothing refers into it.
            unsafe { unmap_view(executable) };
            Err(code)
        }
    });
    // SAFETY: each view holds its own reference to the section, so closing
    // the handle now frees it when the last view goes.
    unsafe { CloseHandle(section) };
    views
}

/// # Safety
/// `view` is the base of a view this crate mapped, and nothing refers into
/// it.
unsafe fn unmap_view(view: NonNull<u8>) {
    // SAFETY: the caller guarantees the view is ours and no longer used.
    let answer = unsafe {
        UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
            Value: view.as_ptr().cast::<c_void>(),
        })
    };
    debug_assert!(answer != 0, "UnmapViewOfFile failed: {}", last_error());
}

/// # Safety
/// `offset..offset + bytes` lies inside the mapping's zone and is
/// page-aligned, and no reference into it is alive.
pub(crate) unsafe fn commit_code(
    mapping: &CodeMapping,
    offset: usize,
    bytes: usize,
) -> Result<(), i32> {
    // SAFETY: the caller guarantees the range is inside the views; committing
    // through one view commits the section's pages behind both, and keeps
    // the contents of pages committed before.
    let answer = unsafe {
        VirtualAlloc(
            mapping
                .writable
                .as_ptr()
                .wrapping_add(offset)
                .cast::<c_void>(),
            bytes,
            MEM_COMMIT,
            PAGE_READWRITE,
        )
    };
    if answer.is_null() {
        return Err(last_error());
    }
    let mut previous: PAGE_PROTECTION_FLAGS = 0;
    // SAFETY: the same pages, now committed, through the executable view;
    // the protection names read and execute only.
    let protected = unsafe {
        VirtualProtect(
            mapping
                .executable
                .as_ptr()
                .wrapping_add(offset)
                .cast::<c_void>(),
            bytes,
            EXECUTABLE_VIEW_PROTECTION,
            &mut previous,
        )
    };
    if protected == 0 {
        Err(last_error())
    } else {
        Ok(())
    }
}

/// Gives back the physical memory behind `offset..offset + bytes`, whose
/// contents are gone. A section's committed pages cannot be decommitted
/// -- `VirtualFree` refuses a view's pages, and `CreateFileMappingW` says
/// a `SEC_RESERVE` section's pages once committed stay so -- so they stay
/// charged against the commit limit; `DiscardVirtualMemory` releases their
/// physical memory, and a later write faults fresh pages in.
///
/// # Safety
/// `offset..offset + bytes` lies inside the mapping's committed zone and
/// is page-aligned, and no reference into it is alive.
pub(crate) unsafe fn decommit_code(
    mapping: &CodeMapping,
    offset: usize,
    bytes: usize,
) -> Result<(), i32> {
    // SAFETY: the caller guarantees the range is inside the committed
    // views and unborrowed; discarding through the writable view discards
    // the section's pages behind both.
    let answer = unsafe {
        DiscardVirtualMemory(
            mapping
                .writable
                .as_ptr()
                .wrapping_add(offset)
                .cast::<c_void>(),
            bytes,
        )
    };
    match i32::try_from(answer) {
        Ok(0) => Ok(()),
        Ok(code) => Err(code),
        Err(_) => Err(i32::MAX),
    }
}

/// # Safety
/// `mapping` is a live mapping this crate made, and nothing refers into it.
pub(crate) unsafe fn unmap_code(mapping: &CodeMapping, _bytes: usize) {
    for view in [mapping.executable, mapping.writable] {
        // SAFETY: the caller guarantees the view is ours and no longer used.
        unsafe { unmap_view(view) };
    }
}

/// Nothing: the two views are separate addresses, so no thread switches
/// between writing and executing.
pub(crate) fn begin_code_write() {}

/// As `begin_code_write`.
pub(crate) fn end_code_write() {}

/// Makes code just written through the writable view visible to the
/// instruction fetch at `address`, its executable address. x64 keeps its
/// instruction cache coherent with stores, so there it does nothing.
///
/// # Safety
/// `address..address + bytes` lies inside a live executable view.
#[cfg(target_arch = "aarch64")]
pub(crate) unsafe fn flush_instruction_cache(address: *const u8, bytes: usize) {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: the caller guarantees the range is mapped; flushing reads
    // nothing and writes nothing the program owns.
    let answer =
        unsafe { FlushInstructionCache(GetCurrentProcess(), address.cast::<c_void>(), bytes) };
    debug_assert!(
        answer != 0,
        "FlushInstructionCache failed: {}",
        last_error()
    );
}

/// As above: x64 needs no flush.
///
/// # Safety
/// None.
#[cfg(not(target_arch = "aarch64"))]
pub(crate) unsafe fn flush_instruction_cache(_address: *const u8, _bytes: usize) {}
