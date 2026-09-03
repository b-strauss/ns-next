#![warn(clippy::all)]

//! Lexer, AST, and parser for the Newspeak3 textual syntax.

pub mod ast;
pub mod diagnostic;
pub mod lexer;
mod parser;
pub mod span;
pub mod token;

pub use ast::Module;
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

/// Parse `src` into a [`Module`]. Returns `Err` on any error-severity
/// diagnostic (from either the lexer or the parser).
pub fn parse(src: &str) -> Result<Module, Vec<Diagnostic>> {
    // Lexer-error gate: the parser assumes well-formed string/symbol lexemes,
    // so a token stream that carried lexer errors must not reach it.
    let tokens = lex(src)?;
    let mut p = parser::Parser::new(src, tokens);
    let module = p.parse_module();
    let diags = p.into_diagnostics();
    if diags.iter().any(|d| d.severity == Severity::Error) {
        Err(diags)
    } else {
        Ok(module)
    }
}
