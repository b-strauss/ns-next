#![warn(clippy::all)]
#![forbid(unsafe_code)]

//! The language server behind ns-next's editor support: diagnostics and
//! semantic tokens for `.ns` files. See `docs/design/editor-support.md`.

pub mod analysis;
pub mod documents;
pub mod semantic_tokens;
