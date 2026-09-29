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
