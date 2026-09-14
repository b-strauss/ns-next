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
    Assign,
    Equals,
    ColonColonEquals,
    Pound,
    LessThan,
    GreaterThan,
    // Literals
    Number,
    String,
    Symbol,
    CharacterConstant,
    // End of input
    EndOfInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}
