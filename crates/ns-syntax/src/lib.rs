#![warn(clippy::all)]

//! Lexer, AST, and parser for the Newspeak3 textual syntax.

pub mod ast;
pub mod diagnostic;
pub mod lexer;
mod parser;
pub mod span;
pub mod token;

pub use diagnostic::{Diagnostic, Severity};
pub use span::Span;
pub use token::{Token, TokenKind};

/// Lex `src` into tokens. Returns `Err` if any error-severity diagnostic was produced.
pub fn lex(src: &str) -> Result<Vec<Token>, Vec<Diagnostic>> {
    let (tokens, diags) = lexer::Lexer::new(src).run();
    if diags.iter().any(|d| d.severity == Severity::Error) {
        Err(diags)
    } else {
        Ok(tokens)
    }
}
