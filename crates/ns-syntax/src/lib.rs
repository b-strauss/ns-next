#![warn(clippy::all)]

//! Lexer, AST, and parser for the Newspeak3 textual syntax.

pub mod diagnostic;
pub mod lexer;
pub mod span;
pub mod token;

pub use diagnostic::{Diagnostic, Severity};
pub use span::Span;
