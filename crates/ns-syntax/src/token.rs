//! Token vocabulary for the Newspeak3 lexer.

use crate::span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    // Names, keywords, and punctuation
    Identifier,
    Keyword,
    SetterKeyword,
    BinarySelector,
    Class,
    Private,
    Public,
    Protected,
    LeftParenthesis,
    RightParenthesis,
    LeftBracket,
    RightBracket,
    LeftBrace,
    RightBrace,
    Bar,
    DoubleBar,
    Caret,
    Dot,
    Semicolon,
    Colon,
    Equals,
    ColonColonEquals,
    Pound,
    LessThan,
    GreaterThan,
    /// `<:`, the mixin application operator (`newspeak-spec.pdf` §6.2).
    MixinOperator,
    // Literals
    Number,
    String,
    Symbol,
    // End of input
    EndOfInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}
