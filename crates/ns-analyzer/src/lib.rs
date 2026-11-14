#![warn(clippy::all)]
#![forbid(unsafe_code)]

//! Everything that looks at Newspeak without running it. The binary
//! `ns-next-analyzer` offers the language server (diagnostics, semantic
//! tokens, fold ranges, document symbols, hovers and formatting for `.ns`
//! files),
//! the token dump the JetBrains plugin's lexer is checked against, `parse`
//! and `parse-corpus`, `disasm`, which compiles a module or decodes an image
//! and prints a body's bytecode, and `inspect`, which prints an image's
//! structure; `fmt` also rewrites sources in the formatter's layout. None
//! of them runs a program's code: a module is compiled and an image
//! decoded, never instantiated or sent anything. Only `disasm` and the
//! language server's breakable lines bootstrap an interpreter, which
//! instantiates the shipped core library and nothing else. This library
//! holds the language server, its hints and the token dump; the other
//! commands live with the binary. See
//! `docs/design/editor-support.md`.

pub mod analysis;
pub mod breakable;
pub mod code_actions;
pub mod doc_tags;
pub mod document_symbols;
pub mod documents;
pub mod file_uri;
pub mod folding;
pub mod hints;
pub mod hover;
pub mod semantic_tokens;
pub mod server;
pub mod token_dump;
