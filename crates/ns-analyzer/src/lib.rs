#![warn(clippy::all)]
#![forbid(unsafe_code)]

//! Everything that looks at Newspeak without running it. The binary
//! `ns-next-analyzer` offers the language server (diagnostics, semantic
//! tokens, fold ranges and document symbols for `.ns` files), the token dump
//! the JetBrains plugin's lexer is checked against, `parse` and
//! `parse-corpus`, `disasm`, which compiles a module or decodes an image and
//! prints a body's bytecode, and `inspect`, which prints an image's
//! structure. None of them starts the VM: a module is compiled and an image
//! decoded, but no Newspeak runs. This library holds the language server and
//! the token dump; the other commands live with the binary. See
//! `docs/design/editor-support.md`.

pub mod analysis;
pub mod document_symbols;
pub mod documents;
pub mod folding;
pub mod semantic_tokens;
pub mod server;
pub mod token_dump;
