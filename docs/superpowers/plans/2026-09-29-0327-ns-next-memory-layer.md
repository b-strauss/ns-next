# The Memory Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `crates/ns-memory`, a crate that reserves one contiguous address range, commits, decommits and protects page ranges of it, and lends accessible committed ranges as `&[u64]` / `&mut [u64]`, on Windows and POSIX -- road step 1 of the road to a high-performance VM.

**Architecture:** A safe `Reservation` type owns one reserved range and a record of which page ranges are committed and how they are protected (`PageMap`, pure Rust). Every operating-system call lives in one private module per platform (`os/windows.rs` over `windows-sys`, `os/posix.rs` over `libc`) behind the same five functions. `ns-memory` is the only crate with `unsafe`; `ns-runtime` keeps `#![forbid(unsafe_code)]` and does not use the crate yet -- road step 3 (the object space) is its first user.

**Tech Stack:** Rust 1.98.0, edition 2021; `windows-sys` 0.61 (features `Win32_System_Memory`, `Win32_System_SystemInformation`, `Win32_Foundation`) on Windows; `libc` 0.2 on Unix. Tests under `cargo-nextest`.

**Spec:** `docs/design/memory.md`, "Planned: the memory layer" (read it first). The road: `docs/OPEN-WORK.md`, "The road to a high-performance VM".

## Global Constraints

- The crate is `crates/ns-memory`, package name `ns-memory`, a library; it joins the workspace members in `Cargo.toml`.
- It is the only crate with `unsafe`; `ns-runtime` and `ns-lsp` keep `#![forbid(unsafe_code)]` unchanged.
- Every `unsafe` block carries a `// SAFETY:` comment; the crate sets `#![deny(unsafe_op_in_unsafe_fn)]` and `#![warn(clippy::undocumented_unsafe_blocks)]`.
- Operating-system calls come only from `windows-sys` (Windows) and `libc` (Unix) -- declaration-only crates, decided with the user; nothing else is added.
- Windows and POSIX are implemented from the start behind one interface.
- A failure the operating system reports is a value (`Err(MemoryError::…)`), never a panic; a malformed argument (misaligned or out-of-bounds range) is also an `Err`, never a panic.
- `protect` and `decommit` take `&mut self`; a lent slice therefore cannot be alive across a change of what is accessible.
- A lent slice only ever covers committed, read-write pages.
- Names are written out (`CLAUDE.md`, "Conventions"; clippy's `min_ident_chars` enforces it): `reservation`, `offset`, `protection`, never `res`, `off`, `prot`, `sz`, `ptr`, `p`.
- Comments explain the decision, not the mechanics.
- Every commit ends with the trailer lines `Co-Authored-By: <the model that wrote it> <noreply@anthropic.com>` and `Claude-Session: https://claude.ai/code/session_0124tQcaVnY7xCNbadqvatoZ`, naming the model that actually wrote the code.
- Conventional commits (`feat:`, `test:`, `docs:`, `chore:`).

## Review Focus

- A range that is not page-aligned, that runs past the reservation's end, or whose end overflows `usize` -- expected: `Err(MemoryError::InvalidRange)`, no panic, no system call.
- Reading a range after `decommit` and a fresh `commit` -- expected: all zeros, not the old contents (a heap would otherwise see stale words).
- `protect(…, NoAccess)` then `words` over any part of it -- expected: `None`; `protect(…, ReadWrite)` afterwards -- expected: the old contents intact.
- Two overlapping ranges passed to `split_words_mut` -- expected: `None` (aliasing `&mut` would be undefined behaviour).
- Many reservations created and dropped in a loop, larger in total than the address space -- expected: every one succeeds, because `Drop` releases.

---

## File Structure

| File | Responsibility |
|---|---|
| `Cargo.toml` (workspace) | adds `crates/ns-memory` to `members` |
| `crates/ns-memory/Cargo.toml` | the package, its two platform dependencies, `[lints] workspace = true` |
| `crates/ns-memory/src/lib.rs` | crate docs and lint attributes; `Reservation`, `Protection`, `MemoryError`, `Operation`; range validation; the public API |
| `crates/ns-memory/src/page_map.rs` | `PageMap`: which byte ranges are committed and with what protection. Pure Rust, no `unsafe` |
| `crates/ns-memory/src/os/mod.rs` | selects `windows` or `posix` by `cfg`, re-exports one set of functions |
| `crates/ns-memory/src/os/windows.rs` | `VirtualAlloc`, `VirtualFree`, `VirtualProtect`, `GetSystemInfo` |
| `crates/ns-memory/src/os/posix.rs` | `mmap`, `mprotect`, `munmap`, `sysconf` |
| `crates/ns-memory/tests/reservation.rs` | integration tests of the public API against the real operating system |
| `CLAUDE.md`, `README.md`, `docs/design/*.md`, `docs/OPEN-WORK.md`, `docs/PROGRESS.md` | the crate list, the gates, the design moved from planned to built (Task 5) |

The public API every task relies on (exact signatures):

```rust
pub struct Reservation { /* private */ }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection { NoAccess, ReadWrite }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation { Reserve, Commit, Decommit, Protect }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    /// The range is empty, not aligned as the operation requires, or not inside the reservation.
    InvalidRange,
    /// `protect` on a range that is not entirely committed.
    NotCommitted,
    /// The operating system refused; `code` is its error number (`GetLastError` or `errno`).
    Refused { operation: Operation, code: i32 },
}

impl Reservation {
    pub fn page_size() -> usize;
    pub fn reserve(bytes: usize) -> Result<Reservation, MemoryError>;
    pub fn size(&self) -> usize;
    pub fn commit(&mut self, range: std::ops::Range<usize>) -> Result<(), MemoryError>;
    pub fn decommit(&mut self, range: std::ops::Range<usize>) -> Result<(), MemoryError>;
    pub fn protect(&mut self, range: std::ops::Range<usize>, protection: Protection) -> Result<(), MemoryError>;
    pub fn words(&self, range: std::ops::Range<usize>) -> Option<&[u64]>;
    pub fn words_mut(&mut self, range: std::ops::Range<usize>) -> Option<&mut [u64]>;
    pub fn split_words_mut<const COUNT: usize>(&mut self, ranges: [std::ops::Range<usize>; COUNT]) -> Option<[&mut [u64]; COUNT]>;
}
impl Drop for Reservation { /* releases the whole range */ }
```

All ranges are **byte offsets from the reservation's base** -- the form the object space's handles take (`docs/design/memory.md`, "Planned: the object space"). `commit`, `decommit` and `protect` need page-aligned ranges; `words`, `words_mut` and `split_words_mut` need 8-byte-aligned ranges.

---

### Task 1: The crate and `PageMap`

**Files:**
- Modify: `Cargo.toml` (workspace `members`)
- Create: `crates/ns-memory/Cargo.toml`
- Create: `crates/ns-memory/src/lib.rs`
- Create: `crates/ns-memory/src/page_map.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub(crate) struct PageMap` with `PageMap::new() -> PageMap`, `fn set(&mut self, range: Range<usize>, state: Option<Protection>)` (`None` = not committed), `fn is_committed(&self, range: Range<usize>) -> bool` (every byte committed, either protection), `fn is_accessible(&self, range: Range<usize>) -> bool` (every byte committed read-write); `pub enum Protection { NoAccess, ReadWrite }` in `lib.rs`.

- [ ] **Step 1: Add the crate to the workspace and create its manifest**

In `Cargo.toml` change the members line to:

```toml
members = ["crates/ns-syntax", "crates/ns-runtime", "crates/ns-cli", "crates/ns-lsp", "crates/ns-memory"]
```

Create `crates/ns-memory/Cargo.toml`:

```toml
[package]
name = "ns-memory"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
description = "Reserved, committed and protected memory for ns-next's object space"

# The operating-system calls, and nothing else: both crates are declarations
# only, with no run-time code of their own. Hand-written declarations would
# have to carry each platform's constants (`MAP_ANON` differs between Linux
# and macOS) and would be wrong only on the platforms nobody here runs
# (docs/design/memory.md, "Decisions").
[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.61", features = ["Win32_Foundation", "Win32_System_Memory", "Win32_System_SystemInformation"] }

[target.'cfg(unix)'.dependencies]
libc = "0.2"

[lints]
workspace = true
```

- [ ] **Step 2: Write `lib.rs` with the crate attributes and `Protection`**

```rust
//! Reserved, committed and protected memory for the object space.
//!
//! One `Reservation` is one contiguous address range, reserved once and
//! committed as a space grows, so that a handle can be a byte offset from a
//! fixed base (docs/design/memory.md, "Planned: the memory layer"). This is
//! the only crate in the workspace with `unsafe` code: every operating-system
//! call is in `os`, and everything this crate hands out is safe to use.
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(clippy::undocumented_unsafe_blocks)]

mod page_map;

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
```

- [ ] **Step 3: Write the failing `PageMap` tests**

Create `crates/ns-memory/src/page_map.rs` with the tests first:

```rust
//! Which byte ranges of a reservation are committed, and how they are
//! protected. The operating system knows this too, but asking it per access
//! would cost a system call; the reservation asks this map before it lends a
//! slice, so a slice can never cover a page that faults.

use std::ops::Range;

use crate::Protection;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_map_has_nothing_committed() {
        let map = PageMap::new();
        assert!(!map.is_committed(0..4096));
        assert!(!map.is_accessible(0..4096));
    }

    #[test]
    fn a_committed_range_is_accessible_and_its_neighbours_are_not() {
        let mut map = PageMap::new();
        map.set(4096..8192, Some(Protection::ReadWrite));
        assert!(map.is_accessible(4096..8192));
        assert!(map.is_accessible(5000..6000));
        assert!(!map.is_accessible(0..4096));
        assert!(!map.is_accessible(4000..5000));
        assert!(!map.is_accessible(8000..9000));
    }

    #[test]
    fn a_no_access_range_is_committed_but_not_accessible() {
        let mut map = PageMap::new();
        map.set(0..8192, Some(Protection::ReadWrite));
        map.set(4096..8192, Some(Protection::NoAccess));
        assert!(map.is_committed(0..8192));
        assert!(map.is_accessible(0..4096));
        assert!(!map.is_accessible(0..8192));
        assert!(!map.is_accessible(4096..5000));
    }

    #[test]
    fn decommitting_the_middle_splits_a_range() {
        let mut map = PageMap::new();
        map.set(0..12288, Some(Protection::ReadWrite));
        map.set(4096..8192, None);
        assert!(map.is_accessible(0..4096));
        assert!(!map.is_committed(4096..8192));
        assert!(map.is_accessible(8192..12288));
        assert!(!map.is_committed(0..12288));
    }

    #[test]
    fn adjacent_ranges_of_one_protection_cover_a_range_spanning_both() {
        let mut map = PageMap::new();
        map.set(0..4096, Some(Protection::ReadWrite));
        map.set(4096..8192, Some(Protection::ReadWrite));
        assert!(map.is_accessible(0..8192));
    }

    #[test]
    fn an_empty_range_is_never_accessible() {
        let mut map = PageMap::new();
        map.set(0..4096, Some(Protection::ReadWrite));
        assert!(!map.is_accessible(100..100));
        assert!(!map.is_committed(100..100));
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo nextest run -p ns-memory`
Expected: compile error, `PageMap` not found.

- [ ] **Step 5: Implement `PageMap`**

Insert above the `tests` module:

```rust
/// Disjoint, sorted byte ranges with their protection; a byte in no range is
/// not committed. Adjacent ranges of one protection are merged, so a range
/// spanning both is found as one.
#[derive(Debug, Default)]
pub(crate) struct PageMap {
    ranges: Vec<(Range<usize>, Protection)>,
}

impl PageMap {
    pub(crate) fn new() -> PageMap {
        PageMap { ranges: Vec::new() }
    }

    /// Records `range` as committed with `state`, or as not committed for
    /// `None`, overriding whatever it was.
    pub(crate) fn set(&mut self, range: Range<usize>, state: Option<Protection>) {
        if range.is_empty() {
            return;
        }
        let mut kept = Vec::with_capacity(self.ranges.len() + 2);
        for (existing, protection) in self.ranges.drain(..) {
            if existing.end <= range.start || existing.start >= range.end {
                kept.push((existing, protection));
                continue;
            }
            if existing.start < range.start {
                kept.push((existing.start..range.start, protection));
            }
            if existing.end > range.end {
                kept.push((range.end..existing.end, protection));
            }
        }
        if let Some(protection) = state {
            kept.push((range, protection));
        }
        kept.sort_by_key(|(existing, _)| existing.start);
        let mut merged: Vec<(Range<usize>, Protection)> = Vec::with_capacity(kept.len());
        for (existing, protection) in kept {
            if let Some((last, last_protection)) = merged.last_mut() {
                if last.end == existing.start && *last_protection == protection {
                    last.end = existing.end;
                    continue;
                }
            }
            merged.push((existing, protection));
        }
        self.ranges = merged;
    }

    /// Whether every byte of `range` is committed, whatever its protection.
    pub(crate) fn is_committed(&self, range: Range<usize>) -> bool {
        self.covered(range, |_| true)
    }

    /// Whether every byte of `range` is committed read-write.
    pub(crate) fn is_accessible(&self, range: Range<usize>) -> bool {
        self.covered(range, |protection| protection == Protection::ReadWrite)
    }

    fn covered(&self, range: Range<usize>, admits: impl Fn(Protection) -> bool) -> bool {
        if range.is_empty() {
            return false;
        }
        let mut position = range.start;
        for (existing, protection) in &self.ranges {
            if existing.end <= position {
                continue;
            }
            if existing.start > position || !admits(*protection) {
                return false;
            }
            position = existing.end;
            if position >= range.end {
                return true;
            }
        }
        false
    }
}
```

`mod page_map;` is already in `lib.rs` (Step 2). Nothing uses `PageMap` until Task 3, so put the attribute on the module declaration there:

```rust
#[allow(dead_code)] // `Reservation` uses it from Task 3 on
mod page_map;
```

(Task 3 removes the `#[allow]`.)

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo nextest run -p ns-memory`
Expected: 6 passed.

- [ ] **Step 7: Lint and format**

Run: `cargo fmt --all` then `cargo clippy --workspace --all-targets`
Expected: no warnings.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock crates/ns-memory
git commit -m "feat: add the ns-memory crate and its page map"
```

---

### Task 2: The operating-system layer

**Files:**
- Create: `crates/ns-memory/src/os/mod.rs`
- Create: `crates/ns-memory/src/os/windows.rs`
- Create: `crates/ns-memory/src/os/posix.rs`
- Modify: `crates/ns-memory/src/lib.rs` (add `mod os;`, `Operation`, `MemoryError`)

**Interfaces:**
- Consumes: `Protection` (Task 1).
- Produces, in `crate::os` (all `pub(crate)`):
  - `fn page_size() -> usize`
  - `fn reservation_granularity() -> usize` (64 KiB on Windows, the page size on POSIX)
  - `unsafe fn reserve(bytes: usize) -> Result<std::ptr::NonNull<u8>, i32>`
  - `unsafe fn commit(address: *mut u8, bytes: usize) -> Result<(), i32>` (read-write)
  - `unsafe fn decommit(address: *mut u8, bytes: usize) -> Result<(), i32>`
  - `unsafe fn protect(address: *mut u8, bytes: usize, protection: Protection) -> Result<(), i32>`
  - `unsafe fn release(address: *mut u8, bytes: usize)`
- Produces, in `lib.rs`: `pub enum Operation { Reserve, Commit, Decommit, Protect }` and `pub enum MemoryError { InvalidRange, NotCommitted, Refused { operation: Operation, code: i32 } }` as in the header of this plan.

- [ ] **Step 1: Add the error types to `lib.rs`**

```rust
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
            MemoryError::InvalidRange => write!(formatter, "the range is not aligned or not inside the reservation"),
            MemoryError::NotCommitted => write!(formatter, "the range is not entirely committed"),
            MemoryError::Refused { operation, code } => {
                write!(formatter, "the operating system refused to {operation:?} memory (error {code})")
            }
        }
    }
}

impl std::error::Error for MemoryError {}
```

Also add, below `mod page_map;`:

```rust
#[allow(dead_code)] // `Reservation` uses it from Task 3 on
mod os;
```

- [ ] **Step 2: Write `os/mod.rs`**

```rust
//! The five operating-system calls a reservation needs, one implementation
//! per platform behind the same functions.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::*;

#[cfg(unix)]
mod posix;
#[cfg(unix)]
pub(crate) use posix::*;
```

- [ ] **Step 3: Write `os/windows.rs`**

```rust
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
    let answer = unsafe { VirtualAlloc(address.cast::<c_void>(), bytes, MEM_COMMIT, PAGE_READWRITE) };
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
pub(crate) unsafe fn protect(address: *mut u8, bytes: usize, protection: Protection) -> Result<(), i32> {
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
```

If a `windows-sys` 0.61 signature differs from the calls above (for example the pointer type `VirtualAlloc` takes), adapt the cast at that call and nothing else; the crate's documentation on docs.rs for 0.61 is authoritative.

- [ ] **Step 4: Write `os/posix.rs`**

```rust
//! `mmap` with `PROT_NONE` reserves, `mprotect` commits and protects, a fixed
//! re-mapping with `PROT_NONE` decommits, `munmap` releases. The re-mapping
//! gives the pages back on Linux and macOS alike, where `madvise` with
//! `MADV_DONTNEED` does not on macOS; it is mapped without `MAP_NORESERVE`, so
//! a later commit is accounted where the kernel accounts commit
//! (docs/design/memory.md, "Planned: the memory layer").

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
pub(crate) unsafe fn protect(address: *mut u8, bytes: usize, protection: Protection) -> Result<(), i32> {
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
    unsafe { libc::munmap(address.cast(), bytes) };
}
```

- [ ] **Step 5: Install the cross-check targets (once per machine)**

Run: `rustup target add x86_64-unknown-linux-gnu aarch64-apple-darwin`
Expected: both installed for the pinned 1.98.0 toolchain. These are only for `cargo check`/`cargo clippy` of the POSIX half; nothing is linked for them.

- [ ] **Step 6: Build on every platform the crate serves**

Run:
```bash
cargo clippy -p ns-memory --all-targets
cargo clippy -p ns-memory --all-targets --target x86_64-unknown-linux-gnu
cargo clippy -p ns-memory --all-targets --target aarch64-apple-darwin
```
Expected: all three succeed with no warnings (the `#[allow(dead_code)]` attributes cover what Task 3 will use).

- [ ] **Step 7: Format and commit**

```bash
cargo fmt --all
git add crates/ns-memory
git commit -m "feat: add ns-memory's operating-system layer for Windows and POSIX"
```

---

### Task 3: `Reservation`: reserve, commit, decommit, protect, release

**Files:**
- Modify: `crates/ns-memory/src/lib.rs`
- Create: `crates/ns-memory/tests/reservation.rs`

**Interfaces:**
- Consumes: `PageMap` (Task 1), `crate::os::*`, `Operation`, `MemoryError` (Task 2).
- Produces: `Reservation::page_size`, `reserve`, `size`, `commit`, `decommit`, `protect`, and `Drop`, with the exact signatures in this plan's header.

- [ ] **Step 1: Write the failing integration tests**

Create `crates/ns-memory/tests/reservation.rs`:

```rust
use ns_memory::{MemoryError, Operation, Protection, Reservation};

fn page() -> usize {
    Reservation::page_size()
}

#[test]
fn a_reservation_is_at_least_as_large_as_asked_and_a_multiple_of_a_page() {
    let reservation = Reservation::reserve(10 * page() + 1).expect("reserve");
    assert!(reservation.size() >= 10 * page() + 1);
    assert_eq!(reservation.size() % page(), 0);
}

#[test]
fn an_empty_reservation_is_an_invalid_range() {
    assert_eq!(Reservation::reserve(0).err(), Some(MemoryError::InvalidRange));
}

#[test]
fn an_impossible_reservation_is_refused_as_a_value() {
    let bytes = usize::MAX - page() + 1;
    match Reservation::reserve(bytes) {
        Err(MemoryError::Refused { operation: Operation::Reserve, .. }) | Err(MemoryError::InvalidRange) => {}
        other => panic!("expected a refusal, got {:?}", other.map(|reservation| reservation.size())),
    }
}

#[test]
fn commit_decommit_and_protect_accept_page_aligned_ranges_inside_the_reservation() {
    let mut reservation = Reservation::reserve(8 * page()).expect("reserve");
    assert_eq!(reservation.commit(0..2 * page()), Ok(()));
    assert_eq!(reservation.protect(page()..2 * page(), Protection::NoAccess), Ok(()));
    assert_eq!(reservation.protect(page()..2 * page(), Protection::ReadWrite), Ok(()));
    assert_eq!(reservation.decommit(0..2 * page()), Ok(()));
}

#[test]
fn a_misaligned_or_outside_range_is_refused_without_a_system_call() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    let size = reservation.size();
    assert_eq!(reservation.commit(1..page()), Err(MemoryError::InvalidRange));
    assert_eq!(reservation.commit(0..page() + 1), Err(MemoryError::InvalidRange));
    assert_eq!(reservation.commit(0..size + page()), Err(MemoryError::InvalidRange));
    assert_eq!(reservation.commit(page()..page()), Err(MemoryError::InvalidRange));
    assert_eq!(reservation.decommit(size..size + page()), Err(MemoryError::InvalidRange));
    let backwards = std::ops::Range { start: 2 * page(), end: page() };
    assert_eq!(reservation.commit(backwards), Err(MemoryError::InvalidRange));
}

#[test]
fn protecting_an_uncommitted_range_is_not_committed() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    assert_eq!(
        reservation.protect(0..2 * page(), Protection::NoAccess),
        Err(MemoryError::NotCommitted)
    );
}

#[test]
fn every_reservation_is_released_when_it_is_dropped() {
    // 64 GiB a time, 4 TiB in all: more than a 47-bit address space holds
    // unless each one is given back.
    for _ in 0..64 {
        let reservation = Reservation::reserve(64 << 30).expect("reserve");
        drop(reservation);
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-memory`
Expected: compile errors, `Reservation` not found.

- [ ] **Step 3: Implement `Reservation`**

In `lib.rs`, remove the two `#[allow(dead_code)]` attributes (on `mod page_map;` and `mod os;`) and add:

```rust
use std::ops::Range;
use std::ptr::NonNull;

use page_map::PageMap;

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
        os::page_size()
    }

    /// Reserves at least `bytes` of address space, committing none of it.
    /// The size is rounded up to the platform's reservation granularity.
    pub fn reserve(bytes: usize) -> Result<Reservation, MemoryError> {
        if bytes == 0 {
            return Err(MemoryError::InvalidRange);
        }
        let granularity = os::reservation_granularity();
        let size = bytes
            .checked_add(granularity - 1)
            .map(|rounded| rounded / granularity * granularity)
            .ok_or(MemoryError::InvalidRange)?;
        // SAFETY: reserving touches no memory the program owns.
        let base = unsafe { os::reserve(size) }
            .map_err(|code| MemoryError::Refused { operation: Operation::Reserve, code })?;
        Ok(Reservation { base, size, pages: PageMap::new() })
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
        unsafe { os::commit(self.address(range.start), range.len()) }
            .map_err(|code| MemoryError::Refused { operation: Operation::Commit, code })?;
        self.pages.set(range, Some(Protection::ReadWrite));
        Ok(())
    }

    /// Gives `range` back to the system; its contents are gone, and a later
    /// commit reads zeros. Takes `&mut self` so that no lent slice survives it.
    pub fn decommit(&mut self, range: Range<usize>) -> Result<(), MemoryError> {
        self.check_pages(&range)?;
        // SAFETY: the range is ours and page-aligned, and `&mut self` means no
        // slice into it is alive.
        unsafe { os::decommit(self.address(range.start), range.len()) }
            .map_err(|code| MemoryError::Refused { operation: Operation::Decommit, code })?;
        self.pages.set(range, None);
        Ok(())
    }

    /// Changes the protection of a committed `range`, keeping its contents.
    /// Takes `&mut self` so that no lent slice survives a change to no-access.
    pub fn protect(&mut self, range: Range<usize>, protection: Protection) -> Result<(), MemoryError> {
        self.check_pages(&range)?;
        if !self.pages.is_committed(range.clone()) {
            return Err(MemoryError::NotCommitted);
        }
        // SAFETY: the range is ours, page-aligned and committed, and `&mut self`
        // means no slice into it is alive.
        unsafe { os::protect(self.address(range.start), range.len(), protection) }
            .map_err(|code| MemoryError::Refused { operation: Operation::Protect, code })?;
        self.pages.set(range, Some(protection));
        Ok(())
    }

    /// A non-empty range inside the reservation, on page boundaries.
    fn check_pages(&self, range: &Range<usize>) -> Result<(), MemoryError> {
        let page = os::page_size();
        let aligned = range.start % page == 0 && range.end % page == 0;
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
        unsafe { os::release(self.base.as_ptr(), self.size) };
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p ns-memory`
Expected: all tests pass (6 page-map tests, 7 reservation tests).

- [ ] **Step 5: Cross-check, lint, format, commit**

Run:
```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo clippy -p ns-memory --all-targets --target x86_64-unknown-linux-gnu
cargo clippy -p ns-memory --all-targets --target aarch64-apple-darwin
```
Expected: no warnings anywhere.

```bash
git add crates/ns-memory
git commit -m "feat: reserve, commit, decommit and protect in ns-memory"
```

---

### Task 4: Lending committed memory as word slices

**Files:**
- Modify: `crates/ns-memory/src/lib.rs`
- Modify: `crates/ns-memory/tests/reservation.rs`

**Interfaces:**
- Consumes: `Reservation` (Task 3), `PageMap::is_accessible`.
- Produces: `Reservation::words`, `words_mut`, `split_words_mut` with the signatures in this plan's header.

- [ ] **Step 1: Write the failing tests**

Append to `crates/ns-memory/tests/reservation.rs`:

```rust
#[test]
fn committed_memory_reads_as_zero_and_keeps_what_is_written() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    let words = reservation.words_mut(0..page()).expect("accessible");
    assert!(words.iter().all(|word| *word == 0));
    words[0] = 0xDEAD_BEEF;
    words[words.len() - 1] = 7;
    let read = reservation.words(0..page()).expect("accessible");
    assert_eq!(read[0], 0xDEAD_BEEF);
    assert_eq!(read[read.len() - 1], 7);
}

#[test]
fn a_slice_covers_only_committed_read_write_memory() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    assert!(reservation.words(0..2 * page()).is_none());
    assert!(reservation.words(page()..2 * page()).is_none());
    assert!(reservation.words(8..16).is_some());
}

#[test]
fn a_misaligned_or_empty_word_range_finds_no_slice() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    assert!(reservation.words(1..9).is_none());
    assert!(reservation.words(0..9).is_none());
    assert!(reservation.words(8..8).is_none());
    let backwards = std::ops::Range { start: 16, end: 8 };
    assert!(reservation.words(backwards).is_none());
}

#[test]
fn a_no_access_range_finds_no_slice_and_keeps_its_contents() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    reservation.words_mut(0..8).expect("accessible")[0] = 42;
    reservation.protect(0..page(), Protection::NoAccess).expect("protect");
    assert!(reservation.words(0..8).is_none());
    reservation.protect(0..page(), Protection::ReadWrite).expect("protect");
    assert_eq!(reservation.words(0..8).expect("accessible")[0], 42);
}

#[test]
fn decommitted_memory_reads_as_zero_when_committed_again() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    reservation.words_mut(0..8).expect("accessible")[0] = 42;
    reservation.decommit(0..page()).expect("decommit");
    assert!(reservation.words(0..8).is_none());
    reservation.commit(0..page()).expect("commit");
    assert_eq!(reservation.words(0..8).expect("accessible")[0], 0);
}

#[test]
fn committing_again_keeps_the_contents() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..page()).expect("commit");
    reservation.words_mut(0..8).expect("accessible")[0] = 42;
    reservation.commit(0..2 * page()).expect("commit");
    assert_eq!(reservation.words(0..8).expect("accessible")[0], 42);
}

#[test]
fn disjoint_ranges_are_lent_at_once_and_overlapping_ones_are_not() {
    let mut reservation = Reservation::reserve(4 * page()).expect("reserve");
    reservation.commit(0..2 * page()).expect("commit");
    let [first, second] = reservation
        .split_words_mut([0..page(), page()..2 * page()])
        .expect("disjoint and accessible");
    first[0] = 1;
    second[0] = 2;
    assert_eq!(reservation.words(0..8).expect("accessible")[0], 1);
    assert_eq!(reservation.words(page()..page() + 8).expect("accessible")[0], 2);
    assert!(reservation.split_words_mut([0..16, 8..24]).is_none());
    assert!(reservation.split_words_mut([0..page(), 2 * page()..3 * page()]).is_none());
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo nextest run -p ns-memory`
Expected: compile errors, no method `words` / `words_mut` / `split_words_mut`.

- [ ] **Step 3: Implement the three lenders**

Add to `impl Reservation` in `lib.rs`:

```rust
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
        let aligned = range.start % 8 == 0 && range.end % 8 == 0;
        let inside = range.start < range.end && range.end <= self.size;
        (aligned && inside && self.pages.is_accessible(range.clone())).then_some(())
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p ns-memory`
Expected: all pass (6 + 14).

- [ ] **Step 5: Cross-check, lint, format, commit**

Run the four commands of Task 3 Step 5; expected no warnings.

```bash
git add crates/ns-memory
git commit -m "feat: lend committed memory as word slices in ns-memory"
```

---

### Task 5: Documentation and gates

**Files:**
- Modify: `CLAUDE.md` (crate list; the `unsafe` sentences; the gates)
- Modify: `README.md` (repository layout)
- Modify: `docs/design/memory.md` ("Planned: the memory layer" becomes a built section; status block; "Decisions")
- Modify: `docs/design/README.md` (the map's memory row)
- Modify: `docs/OPEN-WORK.md` ("Next" and road step 1)
- Modify: `docs/PROGRESS.md` (a milestone section; "Where the next session starts")

**Interfaces:**
- Consumes: the finished crate (Tasks 1-4).
- Produces: documentation that describes the crate as built.

- [ ] **Step 1: `CLAUDE.md`**

- In the opening paragraph, "four crates" becomes "five crates", and add after the `ns-lsp` bullet:

```markdown
- `crates/ns-memory` — the memory layer: one contiguous reservation per heap,
  committed, decommitted and protected by page range, lent as word slices, on
  Windows (`windows-sys`) and POSIX (`libc`). The only crate with `unsafe`;
  every other crate forbids it. Nothing uses it yet: road step 3, the object
  space, is its first user (`docs/design/memory.md`, "The memory layer").
```

- In "The brand is the compiler's, and needs no cast", the sentence "The crate forbids unsafe code (`#![forbid(unsafe_code)]`) and the brand needs none." becomes "`ns-runtime` forbids unsafe code (`#![forbid(unsafe_code)]`) and the brand needs none."
- In "Working in this repository", after the `ns-lsp` gate group, add:

```bash
# --- before every commit that touches crates/ns-memory ---
# once per machine: rustup target add x86_64-unknown-linux-gnu aarch64-apple-darwin
# the POSIX half is checked, not run, since nothing here runs Linux or macOS
cargo clippy -p ns-memory --all-targets --target x86_64-unknown-linux-gnu
cargo clippy -p ns-memory --all-targets --target aarch64-apple-darwin
```

- [ ] **Step 2: `README.md`**

In the layout block, add after `crates/ns-lsp`:

```
crates/ns-memory     reserved, committed and protected memory: the only crate with unsafe
```

- [ ] **Step 3: `docs/design/memory.md`**

- Rename "## Planned: the memory layer" to "## The memory layer" and rewrite its first paragraph as current state: `crates/ns-memory` reserves one contiguous range, commits, decommits and protects page ranges, and lends accessible committed ranges as word slices (`Reservation`, `Protection`, `MemoryError` in `crates/ns-memory/src/lib.rs`; `PageMap` in `page_map.rs`; the calls in `os/windows.rs` and `os/posix.rs`). Keep the bullets, in the present tense where they describe the crate, and keep in the future tense (with a pointer to "Planned: the object space") what describes how the heap will use it (re-borrowing per step, one slice per space, the debug protection of vacated ranges). State that nothing uses the crate yet.
- Update every citation of "Planned: the memory layer" across `docs/` to "The memory layer" (`rg -n "Planned: the memory layer" docs CLAUDE.md README.md` must find nothing afterwards).
- In the status block at the top, list `crates/ns-memory` among the built code.

- [ ] **Step 4: `docs/design/README.md`**

In the map's "Object lifetime, identity, collection" row, move the memory layer from "Planned next" to the built column ("the memory layer, `crates/ns-memory`, which nothing uses yet") and leave the object space, generations and inline allocation planned.

- [ ] **Step 5: `docs/OPEN-WORK.md`**

- "Next" becomes road step 2, the eight-byte value (its plan to be written from `docs/design/object-model.md`, "Planned: an eight-byte value", and `docs/design/core-library.md`, "Planned: large integers").
- In "The road to a high-performance VM", step 1's bullet reads "**Step 1, the memory layer — built** (`crates/ns-memory`; `docs/design/memory.md`, "The memory layer")."

- [ ] **Step 6: `docs/PROGRESS.md`**

- "Where the next session starts": the next piece of work is road step 2, the eight-byte value with `LargeInteger`; point at `OPEN-WORK.md`, "The road to a high-performance VM".
- Add a milestone section at the end, in the style of the others:

```markdown
## The memory layer (2026-09-29)

Plan: [ns-next-memory-layer](superpowers/plans/2026-09-29-0327-ns-next-memory-layer.md).

Road step 1 ([OPEN-WORK.md](OPEN-WORK.md), "The road to a high-performance
VM"): `crates/ns-memory` reserves one contiguous address range, commits,
decommits and protects page ranges, and lends committed read-write ranges as
word slices, on Windows through `windows-sys` and on POSIX through `libc`
([memory.md](design/memory.md), "The memory layer"). It is the only crate with
`unsafe`, and every `unsafe` block states why it holds; `ns-runtime` keeps
`#![forbid(unsafe_code)]`. A failure the operating system reports, and a
malformed range, is a value, never a panic. Nothing uses the crate yet: road
step 3, the object space, is its first user. The POSIX half is checked with
clippy for Linux and macOS targets, since nothing here runs either.
```

- [ ] **Step 7: Check the citations**

Run: `rg -n "Planned: the memory layer" docs CLAUDE.md README.md`
Expected: no match outside `docs/superpowers/plans/`.

- [ ] **Step 8: Commit**

```bash
git add CLAUDE.md README.md docs
git commit -m "docs: describe the memory layer as built"
```

---

### Task 6: End-of-batch gates

**Files:** none changed unless a gate fails.

- [ ] **Step 1: The per-commit gates on the final tree**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo clippy -p ns-memory --all-targets --target x86_64-unknown-linux-gnu
cargo clippy -p ns-memory --all-targets --target aarch64-apple-darwin
cargo nextest run --workspace --exclude ns-lsp
cargo test --workspace --exclude ns-lsp --doc
cargo run -p ns-cli -- parse-corpus corpus/modules
```
Expected: no warnings; every test passes; `parsed 211, failed 0`.

- [ ] **Step 2: The end-of-batch gates (Git Bash)**

```bash
mkdir -p .temp && for mode in MOVES CACHES COLLECT; do
  (env NS_NEXT_STRESS_$mode=1 cargo nextest run --workspace --exclude ns-lsp      > .temp/stress-$mode.log 2>&1 || echo "stress $mode FAILED: .temp/stress-$mode.log") &
done; wait; grep -h Summary .temp/stress-*.log
cargo nextest run -p ns-lsp
cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_MOVES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_CACHES=1 cargo run -p ns-cli -- run-corpus corpus/modules
NS_NEXT_STRESS_COLLECT=1 cargo run -p ns-cli -- run-corpus corpus/modules
```
Expected: every stress summary passes; `ran 6, failed 0, skipped 205` four times. The JetBrains plugin's gradle gates are not run: nothing under `editors/jetbrains`, `crates/ns-lsp` or `crates/ns-syntax` changed.

- [ ] **Step 3: Report**

Report the summaries. No commit unless a gate forced a fix, which then gets its own `fix:` commit.
