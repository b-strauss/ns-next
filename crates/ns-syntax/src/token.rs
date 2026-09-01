//! Token vocabulary for the Newspeak3 lexer.

use crate::span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    // Task 3
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
    // Task 4
    Number,
    String,
    Symbol,
    CharConstant,
    // Task 5
    Eoi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}
