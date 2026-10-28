//! `mmap` with `PROT_NONE` reserves, `mprotect` commits and protects, a fixed
//! re-mapping with `PROT_NONE` decommits, `munmap` releases. The re-mapping
//! gives the pages back on Linux and macOS alike, where `madvise` with
//! `MADV_DONTNEED` does not on macOS; it is mapped without `MAP_NORESERVE`, so
//! a later commit is accounted where the kernel accounts commit
//! (docs/design/memory.md, "The memory layer").
//!
//! The code zone is a shared memory object mapped twice, one view
//! executable and one writable, so no page is ever writable and executable
//! at once -- but on Apple AArch64, where the hardened runtime admits such
//! a page only under `MAP_JIT`, and the per-thread switch keeps a thread
//! from executing what it may write (docs/design/execution.md, "Executable
//! memory").

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

/// The executable view's protection on the platforms that map the zone
/// twice. It never includes write, which is what keeps a stray store from
/// rewriting code that runs.
#[cfg(not(all(target_vendor = "apple", target_arch = "aarch64")))]
const EXECUTABLE_VIEW_PROTECTION: libc::c_int = libc::PROT_READ | libc::PROT_EXEC;

/// The protection the executable view was mapped with includes write. It
/// answers the mapping's request, not a query of the pages: POSIX has no
/// portable query of a page's protection, and a commit here never changes
/// the view's protection, so what was mapped is what the pages have.
#[cfg(all(test, not(all(target_vendor = "apple", target_arch = "aarch64"))))]
pub(crate) fn executable_view_is_writable(_mapping: &CodeMapping, _committed: usize) -> bool {
    EXECUTABLE_VIEW_PROTECTION & libc::PROT_WRITE != 0
}

/// Always `false`, by definition rather than by observation: the one
/// `MAP_JIT` mapping is mapped readable, writable and executable, and what
/// keeps a thread from writing it is the thread's write protection, which
/// only a live `CodeWriter` switches off.
#[cfg(all(test, target_vendor = "apple", target_arch = "aarch64"))]
pub(crate) fn executable_view_is_writable(_mapping: &CodeMapping, _committed: usize) -> bool {
    false
}

/// The views of one code zone, and on Linux the memory object behind them,
/// kept so that a commit can allocate its pages.
///
/// So each zone holds one descriptor for its whole life, closed only by
/// `unmap_code`: `fallocate` needs it, and it is what makes a commit the
/// system cannot back fail as a value on every kernel that has `memfd`
/// rather than as a `SIGBUS` on a later store. Populating the range
/// through the writable view instead (`MADV_POPULATE_WRITE`) would free
/// the descriptor but exists only since Linux 5.14, and an older kernel
/// would fall back to the fault.
pub(crate) struct CodeMapping {
    pub(crate) executable: NonNull<u8>,
    pub(crate) writable: NonNull<u8>,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    descriptor: libc::c_int,
}

/// An anonymous memory object no other process can open: `memfd_create`
/// gives it no name in any namespace.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn shared_memory_object() -> Result<libc::c_int, i32> {
    // SAFETY: the name is a valid C string; the call creates a new object.
    let descriptor = unsafe { libc::memfd_create(c"ns-next-code".as_ptr(), libc::MFD_CLOEXEC) };
    if descriptor < 0 {
        Err(errno())
    } else {
        Ok(descriptor)
    }
}

/// A memory object under a name unique to this process and zone, unlinked
/// at once, so that no other process can open it once it is mapped and
/// none is left behind when the process dies.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    all(target_vendor = "apple", target_arch = "aarch64")
)))]
fn shared_memory_object() -> Result<libc::c_int, i32> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_NAME: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        "/ns-next-code-{}-{}",
        std::process::id(),
        NEXT_NAME.fetch_add(1, Ordering::Relaxed)
    );
    let name = std::ffi::CString::new(name).map_err(|_| libc::EINVAL)?;
    // `shm_open` is variadic on Apple, and a variadic argument narrower
    // than an `int` is not passed as one.
    #[cfg(target_vendor = "apple")]
    let mode = libc::c_uint::from(libc::S_IRUSR | libc::S_IWUSR);
    #[cfg(not(target_vendor = "apple"))]
    let mode = libc::S_IRUSR | libc::S_IWUSR;
    // SAFETY: the name is a valid C string; `O_EXCL` refuses an object
    // someone else created under it.
    let descriptor = unsafe {
        libc::shm_open(
            name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
            mode,
        )
    };
    if descriptor < 0 {
        return Err(errno());
    }
    // SAFETY: unlinking a name we created removes the name only; the open
    // descriptor keeps the object.
    unsafe { libc::shm_unlink(name.as_ptr()) };
    Ok(descriptor)
}

/// # Safety
/// None beyond the call itself; the answer is two fresh views no one else
/// holds. `bytes` is a multiple of the page size.
#[cfg(not(all(target_vendor = "apple", target_arch = "aarch64")))]
pub(crate) unsafe fn map_code(bytes: usize) -> Result<CodeMapping, i32> {
    let descriptor = shared_memory_object()?;
    let close = |code: i32| {
        // SAFETY: the descriptor is ours and nothing else uses it.
        unsafe { libc::close(descriptor) };
        code
    };
    let length = libc::off_t::try_from(bytes).map_err(|_| close(libc::EINVAL))?;
    // SAFETY: sizing an object we just created touches nothing we own; its
    // pages read as zero and take memory only when allocated or written.
    if unsafe { libc::ftruncate(descriptor, length) } != 0 {
        return Err(close(errno()));
    }
    let map = |protection: libc::c_int| {
        // SAFETY: a null hint lets the kernel choose the address; a shared
        // mapping of our own object touches nothing else we own.
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                bytes,
                protection,
                libc::MAP_SHARED,
                descriptor,
                0,
            )
        };
        if address == libc::MAP_FAILED {
            Err(errno())
        } else {
            NonNull::new(address.cast::<u8>()).ok_or(0)
        }
    };
    let executable = map(EXECUTABLE_VIEW_PROTECTION).map_err(close)?;
    let writable = match map(libc::PROT_READ | libc::PROT_WRITE) {
        Ok(writable) => writable,
        Err(code) => {
            // SAFETY: the view was mapped above and nothing refers into it.
            unsafe { libc::munmap(executable.as_ptr().cast(), bytes) };
            return Err(close(code));
        }
    };
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let mapping = CodeMapping {
        executable,
        writable,
        descriptor,
    };
    // The views keep the object alive, and nothing more is asked of it.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let mapping = {
        close(0);
        CodeMapping {
            executable,
            writable,
        }
    };
    Ok(mapping)
}

/// # Safety
/// None beyond the call itself; the answer is a fresh mapping no one else
/// holds. `bytes` is a multiple of the page size.
#[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
pub(crate) unsafe fn map_code(bytes: usize) -> Result<CodeMapping, i32> {
    // SAFETY: a null hint lets the kernel choose the address; the mapping is
    // private and anonymous, so it touches nothing we own. `MAP_JIT` is the
    // only way the hardened runtime admits a page both writable and
    // executable, each thread seeing one or the other.
    let address = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            bytes,
            libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
            libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_JIT,
            -1,
            0,
        )
    };
    if address == libc::MAP_FAILED {
        return Err(errno());
    }
    let base = NonNull::new(address.cast::<u8>()).ok_or(0)?;
    Ok(CodeMapping {
        executable: base,
        writable: base,
    })
}

/// Allocates the pages of `offset..offset + bytes` in the memory object, so
/// that a zone the system cannot back fails here as a value rather than a
/// later store into it with `SIGBUS`. A file system that does not allocate
/// ahead leaves the store to find out, as on other Unix systems.
///
/// # Safety
/// `offset..offset + bytes` lies inside the mapping's zone and is
/// page-aligned.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) unsafe fn commit_code(
    mapping: &CodeMapping,
    offset: usize,
    bytes: usize,
) -> Result<(), i32> {
    let start = libc::off_t::try_from(offset).map_err(|_| libc::EINVAL)?;
    let length = libc::off_t::try_from(bytes).map_err(|_| libc::EINVAL)?;
    // SAFETY: allocating inside our own object's size keeps its contents.
    if unsafe { libc::fallocate(mapping.descriptor, 0, start, length) } == 0 {
        return Ok(());
    }
    match errno() {
        libc::EOPNOTSUPP => Ok(()),
        code => Err(code),
    }
}

/// Nothing: the pages of the memory object or the `MAP_JIT` mapping take
/// memory when first written.
///
/// # Safety
/// None.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub(crate) unsafe fn commit_code(
    _mapping: &CodeMapping,
    _offset: usize,
    _bytes: usize,
) -> Result<(), i32> {
    Ok(())
}

/// # Safety
/// `mapping` is a live mapping of `bytes` this crate made, and nothing refers
/// into it.
pub(crate) unsafe fn unmap_code(mapping: &CodeMapping, bytes: usize) {
    // SAFETY: the caller guarantees the view is ours and no longer used.
    let answer = unsafe { libc::munmap(mapping.executable.as_ptr().cast(), bytes) };
    debug_assert!(answer == 0, "munmap failed: {}", errno());
    if mapping.writable != mapping.executable {
        // SAFETY: as above.
        let answer = unsafe { libc::munmap(mapping.writable.as_ptr().cast(), bytes) };
        debug_assert!(answer == 0, "munmap failed: {}", errno());
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        // SAFETY: the descriptor is ours, and the views it backed are gone.
        unsafe { libc::close(mapping.descriptor) };
    }
}

#[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
thread_local! {
    /// How many writers the calling thread holds. The switch is one per
    /// thread, not one per writer, so only the first writer switches it to
    /// writing and only the last switches it back: a writer that ended
    /// while another lived would otherwise leave the other storing into
    /// pages its thread may only execute.
    static WRITERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Switches the calling thread's view of every `MAP_JIT` page to writable,
/// and so no longer executable, until the matching `end_code_write`.
#[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
pub(crate) fn begin_code_write() {
    let writers = WRITERS.get();
    if writers == 0 {
        // SAFETY: the switch affects only the calling thread's access to
        // `MAP_JIT` pages, and no machine code runs while a writer lives.
        unsafe { libc::pthread_jit_write_protect_np(0) };
    }
    WRITERS.set(writers + 1);
}

/// Switches the calling thread back to executing its `MAP_JIT` pages when
/// its last writer ends.
#[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
pub(crate) fn end_code_write() {
    let writers = WRITERS.get();
    debug_assert!(writers > 0, "a writer ended that never began");
    WRITERS.set(writers.saturating_sub(1));
    if writers == 1 {
        // SAFETY: as for `begin_code_write`.
        unsafe { libc::pthread_jit_write_protect_np(1) };
    }
}

/// Nothing: the two views are separate addresses, so no thread switches
/// between writing and executing.
#[cfg(not(all(target_vendor = "apple", target_arch = "aarch64")))]
pub(crate) fn begin_code_write() {}

/// As `begin_code_write`.
#[cfg(not(all(target_vendor = "apple", target_arch = "aarch64")))]
pub(crate) fn end_code_write() {}

/// Makes code just written visible to the instruction fetch at `address`,
/// its executable address: AArch64's instruction cache is not coherent
/// with its stores, so code written and not flushed may run stale bytes.
///
/// # Safety
/// `address..address + bytes` lies inside a live code mapping.
#[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
pub(crate) unsafe fn flush_instruction_cache(address: *const u8, bytes: usize) {
    // `libc` does not declare it; it is libSystem's.
    extern "C" {
        fn sys_icache_invalidate(start: *mut libc::c_void, length: libc::size_t);
    }
    // SAFETY: the caller guarantees the range is mapped; invalidating the
    // instruction cache over it changes no byte the program owns.
    unsafe { sys_icache_invalidate(address.cast_mut().cast(), bytes) };
}

/// As above.
///
/// # Safety
/// `address..address + bytes` lies inside a live code mapping.
#[cfg(all(target_arch = "aarch64", not(target_vendor = "apple")))]
pub(crate) unsafe fn flush_instruction_cache(address: *const u8, bytes: usize) {
    // `libc` does not declare it; it is the compiler runtime's, which every
    // Rust program links.
    extern "C" {
        fn __clear_cache(start: *mut libc::c_char, end: *mut libc::c_char);
    }
    let start = address.cast_mut();
    // SAFETY: the caller guarantees the range is mapped; cleaning the data
    // cache and invalidating the instruction cache over it changes no byte
    // the program owns.
    unsafe { __clear_cache(start.cast(), start.wrapping_add(bytes).cast()) };
}

/// Nothing: x64 keeps its instruction cache coherent with stores.
///
/// # Safety
/// None.
#[cfg(not(target_arch = "aarch64"))]
pub(crate) unsafe fn flush_instruction_cache(_address: *const u8, _bytes: usize) {}
