#![forbid(unsafe_code)]
#![warn(clippy::all)]

//! What `ns-next` and `ns-next-analyzer` share: loading and resolving a
//! source file, collecting the `.ns` files under a directory and printing
//! diagnostics.
//!
//! It is a crate of its own so that two binaries use one implementation while
//! the language server's dependencies (`lsp-server`, `lsp-types`) stay in
//! `ns-analyzer` and never reach `ns-next`. It depends on `ns-syntax` and
//! `ns-runtime` only.

pub mod processor;
pub mod source;
