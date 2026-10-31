//! `VirtualAlloc` reserves and commits, `VirtualFree` decommits and releases,
//! `VirtualProtect` protects -- Strongtalk's `ReservedSpace`/`VirtualSpace`
//! shape (prior-art/strongtalk/vm/runtime/os_nt.cpp:403-414). Windows accounts
//! every commit, so a commit the system cannot back fails here as a value.
//!
//! The code zone is two address ranges reserved as placeholders, into both
//! of which each chunk's pagefile-backed section is mapped, one view
//! executable and one writable, so no page is ever writable and executable
//! at once (docs/design/execution.md, "Executable memory"), and a chunk
//! given back takes its commit charge with it (`CodeMapping`).

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::OnceLock;

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_PROC_NOT_FOUND, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, VirtualAlloc, VirtualFree, VirtualProtect, MEMORY_MAPPED_VIEW_ADDRESS,
    MEM_COMMIT, MEM_DECOMMIT, MEM_EXTENDED_PARAMETER, MEM_PRESERVE_PLACEHOLDER, MEM_RELEASE,
    MEM_REPLACE_PLACEHOLDER, MEM_RESERVE, MEM_RESERVE_PLACEHOLDER, PAGE_EXECUTE_READ,
    PAGE_EXECUTE_READWRITE, PAGE_NOACCESS, PAGE_PROTECTION_FLAGS, PAGE_READWRITE, SEC_RESERVE,
    UNMAP_VIEW_OF_FILE_FLAGS, VIRTUAL_ALLOCATION_TYPE,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThreadStackLimits, SetThreadStackGuarantee,
};

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

/// The protection of the code zone's executable view. It never includes
/// write, which is what keeps a stray store from rewriting code that runs.
const EXECUTABLE_VIEW_PROTECTION: PAGE_PROTECTION_FLAGS = PAGE_EXECUTE_READ;

/// The code zone is committed and given back in chunks of this many bytes,
/// each a pagefile-backed section of its own mapped into both views. A
/// section's committed pages stay charged until the section goes, so the
/// chunk is the unit in which the zone gives its commit charge back. ZGC
/// maps its heap the same way, in granules of 2 MiB
/// (`prior-art/jdk/src/hotspot/os/windows/gc/z/zMapper_windows.cpp`); the
/// zone's 16 MiB by default and its stubs at the base, a few KiB, take a
/// finer unit, and a chunk is still sixteen times the reservation
/// granularity, so a zone of the default size makes sixteen sections.
pub(crate) const CODE_CHUNK_BYTES: usize = 1 << 20;

/// A code zone's size is a multiple of this: whole chunks.
pub(crate) fn code_reservation_granularity() -> usize {
    CODE_CHUNK_BYTES
}

/// What the code zone commits and gives back at a time: a chunk.
pub(crate) fn code_commit_granularity() -> usize {
    CODE_CHUNK_BYTES
}

/// Each view of a code zone is one mapping per chunk.
pub(crate) fn code_mapping_granularity() -> Option<usize> {
    Some(CODE_CHUNK_BYTES)
}

use crate::code_zone::PLACEHOLDER_REQUIREMENT;

type VirtualAlloc2Function = unsafe extern "system" fn(
    HANDLE,
    *const c_void,
    usize,
    VIRTUAL_ALLOCATION_TYPE,
    u32,
    *mut MEM_EXTENDED_PARAMETER,
    u32,
) -> *mut c_void;
type MapViewOfFile3Function = unsafe extern "system" fn(
    HANDLE,
    HANDLE,
    *const c_void,
    u64,
    usize,
    VIRTUAL_ALLOCATION_TYPE,
    u32,
    *mut MEM_EXTENDED_PARAMETER,
    u32,
) -> MEMORY_MAPPED_VIEW_ADDRESS;
type UnmapViewOfFile2Function =
    unsafe extern "system" fn(HANDLE, MEMORY_MAPPED_VIEW_ADDRESS, UNMAP_VIEW_OF_FILE_FLAGS) -> BOOL;

/// The three calls placeholders take, which `KernelBase.dll` exports from
/// Windows 10 version 1803 on. Resolved at run time, as HotSpot resolves
/// them (`prior-art/jdk/src/hotspot/os/windows/gc/z/zSyscall_windows.cpp:74-81`),
/// so that a system without them refuses the zone when it is made, with
/// the requirement named, rather than the process at its start.
#[derive(Clone, Copy)]
struct PlaceholderCalls {
    virtual_alloc2: VirtualAlloc2Function,
    map_view_of_file3: MapViewOfFile3Function,
    unmap_view_of_file2: UnmapViewOfFile2Function,
}

/// The calls, looked up once per process.
fn placeholder_calls() -> Option<PlaceholderCalls> {
    static CALLS: OnceLock<Option<PlaceholderCalls>> = OnceLock::new();
    *CALLS.get_or_init(|| {
        let name: Vec<u16> = "KernelBase.dll"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: the name is a terminated UTF-16 string; `KernelBase.dll`
        // is loaded into every Win32 process, and the handle is not freed.
        let module = unsafe { GetModuleHandleW(name.as_ptr()) };
        if module.is_null() {
            return None;
        }
        // SAFETY: each name is a terminated C string, the module is live.
        let (virtual_alloc2, map_view_of_file3, unmap_view_of_file2) = unsafe {
            (
                GetProcAddress(module, c"VirtualAlloc2".as_ptr().cast())?,
                GetProcAddress(module, c"MapViewOfFile3".as_ptr().cast())?,
                GetProcAddress(module, c"UnmapViewOfFile2".as_ptr().cast())?,
            )
        };
        // SAFETY: each address is the export of that name, whose signature
        // the Windows SDK declares as the function type it is cast to
        // (`windows-sys`'s own declarations of the three); a function
        // pointer of one ABI cast to another of the same ABI.
        Some(unsafe {
            PlaceholderCalls {
                virtual_alloc2: std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    VirtualAlloc2Function,
                >(virtual_alloc2),
                map_view_of_file3: std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    MapViewOfFile3Function,
                >(map_view_of_file3),
                unmap_view_of_file2: std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    UnmapViewOfFile2Function,
                >(unmap_view_of_file2),
            }
        })
    })
}

/// Whether this system can make a code zone: `Err` names what it lacks.
pub(crate) fn code_zone_support() -> Result<(), &'static str> {
    placeholder_calls()
        .map(|_| ())
        .ok_or(PLACEHOLDER_REQUIREMENT)
}

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

/// The two views of one code zone: two address ranges of the zone's size,
/// each reserved as placeholders, one per chunk (`CODE_CHUNK_BYTES`), and
/// the sections of the chunks committed so far, each mapped into both at
/// the same offset -- read and execute in one, read and write in the
/// other -- in place of its placeholders. ZGC's shape
/// (`zMapper_windows.cpp:62-75`, `:258-312`), and HotSpot's
/// placeholders (`prior-art/jdk/src/hotspot/os/windows/os_windows.cpp:3573-3618`).
pub(crate) struct CodeMapping {
    pub(crate) executable: NonNull<u8>,
    pub(crate) writable: NonNull<u8>,
    calls: PlaceholderCalls,
    /// The section of each chunk, by chunk, null for one not mapped: the
    /// chunk's two views keep it alive, and the handle is closed when they
    /// are unmapped, which frees its pages and their commit charge.
    sections: RefCell<Vec<HANDLE>>,
}

impl CodeMapping {
    /// The executable and the writable address of chunk `chunk`.
    fn chunk_addresses(&self, chunk: usize) -> (*mut u8, *mut u8) {
        let offset = chunk * CODE_CHUNK_BYTES;
        (
            self.executable.as_ptr().wrapping_add(offset),
            self.writable.as_ptr().wrapping_add(offset),
        )
    }
}

/// Reserves `bytes` of address space as one placeholder, split into one
/// placeholder per chunk, so that each chunk's view replaces exactly one.
///
/// # Safety
/// `bytes` is a positive multiple of `CODE_CHUNK_BYTES`.
unsafe fn reserve_placeholders(calls: PlaceholderCalls, bytes: usize) -> Result<NonNull<u8>, i32> {
    // SAFETY: reserving a placeholder with a null address asks the system to
    // pick one; it touches no memory the program owns.
    let base = unsafe {
        (calls.virtual_alloc2)(
            GetCurrentProcess(),
            std::ptr::null(),
            bytes,
            MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
            PAGE_NOACCESS,
            std::ptr::null_mut(),
            0,
        )
    };
    let base = NonNull::new(base.cast::<u8>()).ok_or_else(last_error)?;
    // Each split takes the next chunk off the front of the placeholder that
    // is left; the last chunk is what remains (`zMapper_windows.cpp:258-268`).
    for chunk in 0..bytes / CODE_CHUNK_BYTES - 1 {
        // SAFETY: the range lies at the front of the placeholder just
        // reserved, which nothing maps; splitting changes only placeholders.
        let split = unsafe {
            VirtualFree(
                base.as_ptr()
                    .wrapping_add(chunk * CODE_CHUNK_BYTES)
                    .cast::<c_void>(),
                CODE_CHUNK_BYTES,
                MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
            )
        };
        if split == 0 {
            let code = last_error();
            // SAFETY: the placeholders are ours and nothing maps them.
            unsafe { release_placeholders(base, bytes) };
            return Err(code);
        }
    }
    Ok(base)
}

/// Releases the placeholders of `bytes` from `base`, one per chunk.
///
/// # Safety
/// `base` is a range `reserve_placeholders` made, `bytes` long, whose
/// chunks are all placeholders again: no view is mapped into it.
unsafe fn release_placeholders(base: NonNull<u8>, bytes: usize) {
    for chunk in 0..bytes / CODE_CHUNK_BYTES {
        // SAFETY: the caller guarantees each chunk is a placeholder of ours
        // that nothing maps; a placeholder never split off is released by
        // the first call and the later ones fail harmlessly.
        unsafe {
            VirtualFree(
                base.as_ptr()
                    .wrapping_add(chunk * CODE_CHUNK_BYTES)
                    .cast::<c_void>(),
                0,
                MEM_RELEASE,
            )
        };
    }
}

/// # Safety
/// None beyond the call itself; the answer is two fresh ranges no one else
/// holds. `bytes` is a positive multiple of `CODE_CHUNK_BYTES`.
pub(crate) unsafe fn map_code(bytes: usize) -> Result<CodeMapping, i32> {
    // `code_zone_support` refused the zone before this is called.
    let calls = placeholder_calls().ok_or(ERROR_PROC_NOT_FOUND as i32)?;
    // SAFETY: `bytes` is a positive multiple of a chunk, as the caller
    // guarantees.
    let executable = unsafe { reserve_placeholders(calls, bytes) }?;
    // SAFETY: as above.
    let writable = match unsafe { reserve_placeholders(calls, bytes) } {
        Ok(writable) => writable,
        Err(code) => {
            // SAFETY: the executable range was reserved just above and
            // nothing maps it.
            unsafe { release_placeholders(executable, bytes) };
            return Err(code);
        }
    };
    Ok(CodeMapping {
        executable,
        writable,
        calls,
        sections: RefCell::new(vec![std::ptr::null_mut(); bytes / CODE_CHUNK_BYTES]),
    })
}

/// Maps chunk `chunk`: a fresh section, `SEC_RESERVE` as ZGC creates its
/// paging file mappings (`zMapper_windows.cpp:90-113`), mapped into both
/// views in place of their placeholders, then committed through the
/// writable view and protected read and execute in the executable one.
///
/// # Safety
/// `chunk` lies inside the mapping and is not mapped.
unsafe fn map_chunk(mapping: &CodeMapping, chunk: usize) -> Result<HANDLE, i32> {
    let (executable, writable) = mapping.chunk_addresses(chunk);
    let size = u32::try_from(CODE_CHUNK_BYTES).map_err(|_| 0)?;
    // SAFETY: a pagefile-backed, unnamed section touches no memory the
    // program owns; its maximum protection admits both views.
    let section = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_EXECUTE_READWRITE | SEC_RESERVE,
            0,
            size,
            std::ptr::null(),
        )
    };
    if section.is_null() {
        return Err(last_error());
    }
    let map = |address: *mut u8, protection: PAGE_PROTECTION_FLAGS| {
        // SAFETY: `address` is the start of one of our chunk placeholders,
        // a chunk long, which the view replaces; the section is live.
        let view = unsafe {
            (mapping.calls.map_view_of_file3)(
                section,
                GetCurrentProcess(),
                address.cast::<c_void>(),
                0,
                CODE_CHUNK_BYTES,
                MEM_REPLACE_PLACEHOLDER,
                protection,
                std::ptr::null_mut(),
                0,
            )
        };
        if view.Value.is_null() {
            Err(last_error())
        } else {
            Ok(())
        }
    };
    let mapped = map(executable, EXECUTABLE_VIEW_PROTECTION).and_then(|()| {
        map(writable, PAGE_READWRITE).inspect_err(|_| {
            // SAFETY: the executable view was mapped just above.
            unsafe { unmap_chunk_view(mapping, executable) };
        })
    });
    if let Err(code) = mapped {
        // SAFETY: the handle is the section just created; no view of it
        // is left.
        unsafe { CloseHandle(section) };
        return Err(code);
    }
    // SAFETY: both views map the chunk now, a chunk long each.
    match unsafe { commit_chunk(executable, writable) } {
        Ok(()) => Ok(section),
        Err(code) => {
            // SAFETY: both views were mapped above, the section is ours.
            unsafe { unmap_chunk(mapping, executable, writable, section) };
            Err(code)
        }
    }
}

/// Commits a mapped chunk's pages through the writable view, and protects
/// them read and execute in the executable one.
///
/// # Safety
/// Both addresses start a chunk mapped from the same section.
unsafe fn commit_chunk(executable: *mut u8, writable: *mut u8) -> Result<(), i32> {
    // SAFETY: the range is the writable view of one mapped chunk;
    // committing through one view commits the section's pages behind both.
    let answer = unsafe {
        VirtualAlloc(
            writable.cast::<c_void>(),
            CODE_CHUNK_BYTES,
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
            executable.cast::<c_void>(),
            CODE_CHUNK_BYTES,
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

/// Unmaps one view of a chunk, leaving its placeholder.
///
/// # Safety
/// `address` starts a view of a chunk of `mapping`, which nothing refers
/// into.
unsafe fn unmap_chunk_view(mapping: &CodeMapping, address: *mut u8) {
    // SAFETY: the caller guarantees the view is ours and unused.
    let answer = unsafe {
        (mapping.calls.unmap_view_of_file2)(
            GetCurrentProcess(),
            MEMORY_MAPPED_VIEW_ADDRESS {
                Value: address.cast::<c_void>(),
            },
            MEM_PRESERVE_PLACEHOLDER,
        )
    };
    debug_assert!(answer != 0, "UnmapViewOfFile2 failed: {}", last_error());
}

/// Unmaps both views of a chunk, leaving their placeholders, and closes
/// its section, which frees its pages and their commit charge.
///
/// # Safety
/// Both addresses start the views of one chunk of `mapping` mapped from
/// `section`, which nothing refers into.
unsafe fn unmap_chunk(
    mapping: &CodeMapping,
    executable: *mut u8,
    writable: *mut u8,
    section: HANDLE,
) {
    // SAFETY: the caller guarantees both views are ours and unused, and
    // the handle is their section's, which no other view keeps.
    unsafe {
        unmap_chunk_view(mapping, executable);
        unmap_chunk_view(mapping, writable);
        CloseHandle(section);
    }
}

/// Maps and commits every chunk of `offset..offset + bytes` not mapped
/// yet: a chunk is committed whole when it is mapped.
///
/// # Safety
/// `offset..offset + bytes` lies inside the mapping's zone and is
/// chunk-aligned, and no reference into it is alive.
pub(crate) unsafe fn commit_code(
    mapping: &CodeMapping,
    offset: usize,
    bytes: usize,
) -> Result<(), i32> {
    let mut sections = mapping.sections.borrow_mut();
    for chunk in offset / CODE_CHUNK_BYTES..(offset + bytes).div_ceil(CODE_CHUNK_BYTES) {
        if sections[chunk].is_null() {
            // SAFETY: the chunk lies inside the mapping, and is not mapped.
            sections[chunk] = unsafe { map_chunk(mapping, chunk) }?;
        }
    }
    Ok(())
}

/// Gives back every mapped chunk of `offset..offset + bytes`: both its
/// views unmapped back to placeholders and its section closed, so that
/// its pages and their commit charge go. Answers true: the system took
/// them back.
///
/// # Safety
/// `offset..offset + bytes` lies inside the mapping's zone and is
/// chunk-aligned, and no reference into it is alive.
pub(crate) unsafe fn decommit_code(
    mapping: &CodeMapping,
    offset: usize,
    bytes: usize,
) -> Result<bool, i32> {
    let mut sections = mapping.sections.borrow_mut();
    for chunk in offset / CODE_CHUNK_BYTES..(offset + bytes).div_ceil(CODE_CHUNK_BYTES) {
        let section = std::mem::replace(&mut sections[chunk], std::ptr::null_mut());
        if !section.is_null() {
            let (executable, writable) = mapping.chunk_addresses(chunk);
            // SAFETY: the chunk is mapped from `section`, and the caller
            // guarantees nothing refers into it.
            unsafe { unmap_chunk(mapping, executable, writable, section) };
        }
    }
    Ok(true)
}

/// # Safety
/// `mapping` is a live mapping this crate made, `bytes` long, and nothing
/// refers into it.
pub(crate) unsafe fn unmap_code(mapping: &CodeMapping, bytes: usize) {
    // SAFETY: the whole zone is ours and unused, as the caller guarantees.
    let _ = unsafe { decommit_code(mapping, 0, bytes) };
    for base in [mapping.executable, mapping.writable] {
        // SAFETY: every chunk is a placeholder again, mapped by nothing.
        unsafe { release_placeholders(base, bytes) };
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
