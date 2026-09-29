//! Reserved, committed and protected memory for the object space.
//!
//! One `Reservation` is one contiguous address range, reserved once and
//! committed as a space grows, so that a handle can be a byte offset from a
//! fixed base (docs/design/memory.md, "Planned: the memory layer"). This is
//! the only crate in the workspace with `unsafe` code: every operating-system
//! call is in `os`, and everything this crate hands out is safe to use.
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(clippy::undocumented_unsafe_blocks)]

#[allow(dead_code)] // `Reservation` uses it from Task 3 on
mod page_map;

// The module is named for the operating-system layer it holds, which is its
// file's name (a name bound to a file keeps it).
#[allow(dead_code, clippy::min_ident_chars)] // `Reservation` uses it from Task 3 on
mod os;

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
