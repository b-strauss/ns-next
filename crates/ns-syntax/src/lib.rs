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
/// produced, or if `source` is longer than a `Span` can address.
pub fn lex(source: &str) -> Result<Vec<Token>, Vec<Diagnostic>> {
    if let Some(diagnostic) = source_too_long(source.len()) {
        return Err(vec![diagnostic]);
    }
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
/// diagnostic (from either the lexer or the parser), or if `source` is longer
/// than a `Span` can address.
pub fn parse(source: &str) -> Result<Module, Vec<Diagnostic>> {
    // Lexer-error gate: the parser assumes well-formed string/symbol lexemes,
    // so a token stream that carried lexer errors must not reach it. `lex`
    // also refuses a source too long to address.
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

/// The diagnostic refusing a source of `length` bytes when a `Span`, whose
/// offsets are `u32`, cannot address its end. Lexed, such a source would
/// wrap its offsets into spans that point at the wrong text, and every count
/// later bounded by source -- the image format's instruction and statement
/// counts among them -- would lose that bound. Checked on the length alone,
/// before a byte is lexed; public for a tool that runs the lexer itself
/// (`ns-next-lsp dump-tokens`).
pub fn source_too_long(length: usize) -> Option<Diagnostic> {
    match u32::try_from(length) {
        Ok(_) => None,
        Err(_) => Some(Diagnostic::error(
            Span::new(0, 0),
            format!(
                "source too long: {length} bytes, more than the {} a source may have",
                u32::MAX
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::source_too_long;

    /// A source of exactly `u32::MAX` bytes ends at the last offset a `Span`
    /// holds; one byte more is refused, without allocating either.
    #[test]
    fn a_source_past_four_gibibytes_is_refused() {
        let longest = u32::MAX as usize;
        assert!(source_too_long(0).is_none());
        assert!(source_too_long(longest).is_none());
        let Some(diagnostic) = source_too_long(longest + 1) else {
            panic!("a source of 2^32 bytes is refused");
        };
        assert_eq!(
            diagnostic.message,
            "source too long: 4294967296 bytes, more than the 4294967295 a source may have"
        );
        assert_eq!(diagnostic.severity, super::Severity::Error);
    }
}
