#![warn(clippy::all)]

//! Lexer, AST, and parser for the Newspeak3 textual syntax.

pub mod ast;
mod attach;
pub mod comment;
pub mod diagnostic;
pub mod lexer;
mod parser;
mod positions;
mod printer;
pub mod span;
pub mod token;

pub use ast::Module;
pub use comment::{Comment, CommentKind};
pub use diagnostic::{Diagnostic, Severity};
pub use printer::{format, print_module, LineEnding};
pub use span::Span;
pub use token::{Token, TokenKind};

/// Lex `source` into tokens. Returns `Err` if any error-severity diagnostic was
/// produced, or if `source` is longer than a `Span` can address.
pub fn lex(source: &str) -> Result<Vec<Token>, Vec<Diagnostic>> {
    if let Some(diagnostic) = source_too_long(source.len()) {
        return Err(vec![diagnostic]);
    }
    let (tokens, diagnostics) = lexer::Lexer::new(source).run();
    if has_error(&diagnostics) {
        Err(diagnostics)
    } else {
        Ok(tokens)
    }
}

/// Parse `source` into a [`Module`], every comment attached to the node it
/// belongs to (`ast::AttachedComment`). Returns `Err` on any error-severity
/// diagnostic (from either the lexer or the parser), or if `source` is
/// longer than a `Span` can address.
pub fn parse(source: &str) -> Result<Module, Vec<Diagnostic>> {
    if let Some(diagnostic) = source_too_long(source.len()) {
        return Err(vec![diagnostic]);
    }
    // Lexer-error gate: the parser assumes well-formed string/symbol lexemes,
    // so a token stream that carried lexer errors must not reach it.
    let lexed = lexer::Lexer::new(source).run_keeping_comments();
    if has_error(&lexed.diagnostics) {
        return Err(lexed.diagnostics);
    }
    let mut parser = parser::Parser::new(source, lexed.tokens);
    let mut module = parser.parse_module();
    let (tokens, diagnostics) = parser.into_parts();
    if has_error(&diagnostics) {
        return Err(diagnostics);
    }
    attach::attach_comments(&mut module, &tokens, &lexed.comments, source);
    Ok(module)
}

fn has_error(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
}

/// What [`parse_for_editor`] answers: a module for every source, recovered
/// where the source has errors, with what an editor reads beside it.
pub struct EditorParse {
    /// The lexer's tokens, before the parser splits any.
    pub tokens: Vec<Token>,
    pub module: Module,
    /// Every comment, in source order, consecutive `///` lines as one.
    pub comments: Vec<Comment>,
    /// The lexer's diagnostics, then the parser's.
    pub diagnostics: Vec<Diagnostic>,
}

/// Parse `source` for an editor, which shows a file while it is typed: a
/// lexer error does not stop the parse, and the module is answered even
/// when the source has errors, its comments attached as [`parse`] attaches
/// them. Nothing compiles such a module; [`parse`]
/// is the entry for that.
pub fn parse_for_editor(source: &str) -> EditorParse {
    if let Some(diagnostic) = source_too_long(source.len()) {
        return EditorParse {
            tokens: Vec::new(),
            module: empty_module(),
            comments: Vec::new(),
            diagnostics: vec![diagnostic],
        };
    }
    let lexed = lexer::Lexer::new(source).run_keeping_comments();
    let mut parser = parser::Parser::new(source, lexed.tokens.clone());
    let mut module = parser.parse_module();
    let (tokens, parser_diagnostics) = parser.into_parts();
    attach::attach_comments(&mut module, &tokens, &lexed.comments, source);
    let mut diagnostics = lexed.diagnostics;
    diagnostics.extend(parser_diagnostics);
    EditorParse {
        tokens: lexed.tokens,
        module,
        comments: lexed.comments,
        diagnostics,
    }
}

/// The module of a source too long to lex: a root class with no name and
/// nothing in it.
fn empty_module() -> Module {
    let span = Span::new(0, 0);
    let body = ast::ClassBody {
        header_slots: Vec::new(),
        initializer_expressions: Vec::new(),
        members: Vec::new(),
        delimiters: None,
        slot_delimiters: None,
        comments: Vec::new(),
        span,
    };
    Module {
        language_id: None,
        category: None,
        root_class: ast::ClassDeclaration {
            access: None,
            name: ast::Identifier {
                name: String::new(),
                span,
            },
            type_parameters: None,
            factory: None,
            factory_return_type: None,
            factory_where_clause: None,
            superclass: None,
            mixin_applications: Vec::new(),
            instance_side: body,
            class_side: None,
            comments: Vec::new(),
            span,
        },
        comments: Vec::new(),
        span,
    }
}

/// The diagnostic refusing a source of `length` bytes when a `Span`, whose
/// offsets are `u32`, cannot address its end. Lexed, such a source would
/// wrap its offsets into spans that point at the wrong text, and every count
/// later bounded by source -- the image format's instruction and statement
/// counts among them -- would lose that bound. Checked on the length alone,
/// before a byte is lexed; public for a tool that runs the lexer itself
/// (`ns-next-analyzer dump-tokens`).
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
