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
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
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
    CharConstant,
    // End of input
    Eoi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}
