#![warn(clippy::all)]
#![forbid(unsafe_code)]

//! The language server behind ns-next's editor support: diagnostics,
//! semantic tokens, fold ranges and document symbols for `.ns` files. See
//! `docs/design/editor-support.md`.

pub mod analysis;
pub mod document_symbols;
pub mod documents;
pub mod folding;
pub mod semantic_tokens;
pub mod server;
pub mod token_dump;
