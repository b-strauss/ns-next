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

/// The lowest address of the running thread's stack, as its attributes
/// report it (`pthread_getattr_np`; since glibc 2.27 the guard lies below
/// what it reports). `None` when the call fails.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn thread_stack_lowest_usable_address() -> Option<usize> {
    // SAFETY: `pthread_attr_t` is plain data, which `pthread_getattr_np`
    // initializes whole.
    let mut attributes: libc::pthread_attr_t = unsafe { std::mem::zeroed() };
    // SAFETY: the call fills the attributes of the running thread.
    if unsafe { libc::pthread_getattr_np(libc::pthread_self(), &mut attributes) } != 0 {
        return None;
    }
    lowest_address_of(&mut attributes)
}

/// The lowest address of the running thread's stack, as its attributes
/// report it (`pthread_attr_get_np` into attributes made by
/// `pthread_attr_init`). `None` when a call fails.
#[cfg(any(target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd"))]
pub(crate) fn thread_stack_lowest_usable_address() -> Option<usize> {
    // SAFETY: `pthread_attr_t` is plain data, which `pthread_attr_init`
    // initializes.
    let mut attributes: libc::pthread_attr_t = unsafe { std::mem::zeroed() };
    // SAFETY: the attributes are the caller's own, initialized here.
    if unsafe { libc::pthread_attr_init(&mut attributes) } != 0 {
        return None;
    }
    // SAFETY: the call fills initialized attributes with the running
    // thread's.
    if unsafe { libc::pthread_attr_get_np(libc::pthread_self(), &mut attributes) } != 0 {
        // SAFETY: the attributes were initialized above.
        unsafe { libc::pthread_attr_destroy(&mut attributes) };
        return None;
    }
    lowest_address_of(&mut attributes)
}

/// The stack's lowest address in `attributes`, which are initialized and
/// destroyed here.
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd"
))]
fn lowest_address_of(attributes: &mut libc::pthread_attr_t) -> Option<usize> {
    let mut address: *mut libc::c_void = std::ptr::null_mut();
    let mut size: libc::size_t = 0;
    // SAFETY: the attributes are initialized, and the call writes the two
    // words it is given.
    let outcome = unsafe { libc::pthread_attr_getstack(attributes, &mut address, &mut size) };
    // SAFETY: the attributes are initialized, and not used again.
    unsafe { libc::pthread_attr_destroy(attributes) };
    (outcome == 0 && !address.is_null() && size > 0).then(|| address.addr())
}

/// The lowest address of the running thread's stack: its highest
/// (`pthread_get_stackaddr_np`) less its size (`pthread_get_stacksize_np`).
#[cfg(target_vendor = "apple")]
pub(crate) fn thread_stack_lowest_usable_address() -> Option<usize> {
    // SAFETY: both read the running thread's own record.
    let (highest, size) = unsafe {
        let thread = libc::pthread_self();
        (
            libc::pthread_get_stackaddr_np(thread).addr(),
            libc::pthread_get_stacksize_np(thread),
        )
    };
    (highest != 0 && size > 0)
        .then(|| highest.checked_sub(size))
        .flatten()
}

/// Unknown on every other system: the JIT runs interpreted there.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_vendor = "apple"
)))]
pub(crate) fn thread_stack_lowest_usable_address() -> Option<usize> {
    None
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
/// only a `CodeWriter`'s write switches off, for the write.
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
    // A signal pending during the allocation interrupts it (tmpfs, behind
    // a memfd, checks for one) without saying anything about the memory,
    // so the call is made again rather than the commit refused.
    loop {
        // SAFETY: allocating inside our own object's size keeps its contents.
        if unsafe { libc::fallocate(mapping.descriptor, 0, start, length) } == 0 {
            return Ok(());
        }
        match errno() {
            libc::EINTR => {}
            libc::EOPNOTSUPP => return Ok(()),
            code => return Err(code),
        }
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

/// Gives back the memory behind `offset..offset + bytes` of the memory
/// object, whose contents are gone and read as zeros through both views
/// until written again: a hole punched into it, its size kept. A file
/// system without holes gives nothing back.
///
/// # Safety
/// `offset..offset + bytes` lies inside the mapping's zone and is
/// page-aligned, and no reference into it is alive.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) unsafe fn decommit_code(
    mapping: &CodeMapping,
    offset: usize,
    bytes: usize,
) -> Result<(), i32> {
    let start = libc::off_t::try_from(offset).map_err(|_| libc::EINVAL)?;
    let length = libc::off_t::try_from(bytes).map_err(|_| libc::EINVAL)?;
    loop {
        // SAFETY: a hole punched inside our own object's size keeps its
        // size and frees only the range, which nothing refers into.
        let answer = unsafe {
            libc::fallocate(
                mapping.descriptor,
                libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE,
                start,
                length,
            )
        };
        if answer == 0 {
            return Ok(());
        }
        match errno() {
            libc::EINTR => {}
            libc::EOPNOTSUPP => return Ok(()),
            code => return Err(code),
        }
    }
}

/// Lets the system take back the memory behind `offset..offset + bytes`
/// of the one `MAP_JIT` mapping, whose contents are undefined until
/// written again.
///
/// # Safety
/// `offset..offset + bytes` lies inside the mapping's zone and is
/// page-aligned, and no reference into it is alive.
#[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
pub(crate) unsafe fn decommit_code(
    mapping: &CodeMapping,
    offset: usize,
    bytes: usize,
) -> Result<(), i32> {
    // SAFETY: the caller guarantees the range is ours and unborrowed;
    // `MADV_FREE` changes no mapping and only lets its pages go.
    let answer = unsafe {
        libc::madvise(
            mapping.executable.as_ptr().wrapping_add(offset).cast(),
            bytes,
            libc::MADV_FREE,
        )
    };
    if answer == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

/// Nothing: a POSIX shared memory object has no portable way to give back
/// pages inside its size, so they stay until the zone is unmapped.
///
/// # Safety
/// None.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    all(target_vendor = "apple", target_arch = "aarch64")
)))]
pub(crate) unsafe fn decommit_code(
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
    /// How many writes the calling thread is in: one at most, since a
    /// write calls nothing that writes, but the switch is one per thread,
    /// so counting keeps a write nested inside another -- should one ever
    /// be -- from switching the thread back under the outer one.
    static WRITERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Switches the calling thread's view of every `MAP_JIT` page to writable,
/// and so no longer executable, until the matching `end_code_write`.
#[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
pub(crate) fn begin_code_write() {
    let writers = WRITERS.get();
    if writers == 0 {
        // SAFETY: the switch affects only the calling thread's access to
        // `MAP_JIT` pages, and the thread runs no machine code while it
        // writes: a write is one call that switches back before it returns.
        unsafe { libc::pthread_jit_write_protect_np(0) };
    }
    WRITERS.set(writers + 1);
}

/// Switches the calling thread back to executing its `MAP_JIT` pages when
/// its outermost write ends.
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
