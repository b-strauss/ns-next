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

/// Lex `source` into tokens. Returns `Err` if any error-severity diagnostic was
/// produced.
pub fn lex(source: &str) -> Result<Vec<Token>, Vec<Diagnostic>> {
    let (tokens, diagnostics) = lexer::Lexer::new(source).run();
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        Err(diagnostics)
    } else {
        Ok(tokens)
    }
}

/// Parse `source` into a [`Module`]. Returns `Err` on any error-severity
/// diagnostic (from either the lexer or the parser).
pub fn parse(source: &str) -> Result<Module, Vec<Diagnostic>> {
    // Lexer-error gate: the parser assumes well-formed string/symbol lexemes,
    // so a token stream that carried lexer errors must not reach it.
    let tokens = lex(source)?;
    let mut parser = parser::Parser::new(source, tokens);
    let module = parser.parse_module();
    let diagnostics = parser.into_diagnostics();
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        Err(diagnostics)
    } else {
        Ok(module)
    }
}
