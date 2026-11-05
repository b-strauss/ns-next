//! The optimizing compiler (`docs/design/execution.md`, "Planned: the
//! optimizing JIT"): it compiles from a snapshot of a body, taken at the
//! run loop's safepoint, so that its work reads nothing the interpreter can
//! change.

pub(crate) mod snapshot;
