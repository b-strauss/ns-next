//! Recursive-descent + Pratt parser for Newspeak3.

// Scaffold: the module is `mod parser;` (private) and, outside of tests, nothing
// in the crate calls into it yet. Tasks 8+ wire it up and add the public entry
// point. Until then the helpers and primary-expression code are only exercised
// by this module's own tests, so silence dead-code warnings crate-wide here.
#![allow(dead_code)]

mod expr;

use crate::ast::*;
use crate::diagnostic::Diagnostic;
use crate::token::{Token, TokenKind};

pub struct Parser<'a> {
    src: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    diags: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str, tokens: Vec<Token>) -> Parser<'a> {
        Parser {
            src,
            tokens,
            pos: 0,
            diags: Vec::new(),
        }
    }

    fn cur(&self) -> Token {
        self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn peek(&self) -> TokenKind {
        self.cur().kind
    }

    // Unused until Task 8's message-send parsing needs one-token lookahead.
    fn peek2(&self) -> TokenKind {
        self.tokens
            .get(self.pos + 1)
            .map_or(TokenKind::Eoi, |t| t.kind)
    }

    fn at(&self, k: TokenKind) -> bool {
        self.peek() == k
    }

    fn bump(&mut self) -> Token {
        let t = self.cur();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    // Unused until Task 8; kept here as part of the cursor helper set.
    fn eat(&mut self, k: TokenKind) -> bool {
        if self.at(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, k: TokenKind) -> Option<Token> {
        if self.at(k) {
            Some(self.bump())
        } else {
            let t = self.cur();
            self.diags.push(Diagnostic::error(
                t.span,
                format!("expected {k:?}, found {:?}", t.kind),
            ));
            None
        }
    }

    // Returns `&'a str` (not the brief's elided lifetime): the slice borrows
    // `self.src: &'a str`, not `&self`, so later tasks can call
    // `self.text(self.bump())` without a borrow-checker conflict.
    fn text(&self, tok: Token) -> &'a str {
        tok.span.text(self.src)
    }

    fn ident(&mut self) -> Ident {
        let t = self.bump();
        Ident {
            name: self.text(t).to_string(),
            span: t.span,
        }
    }

    pub(crate) fn parse_expression(&mut self) -> Expr {
        // Task 8 replaces this with full message-send parsing.
        self.parse_primary()
    }
}

#[cfg(test)]
impl<'a> Parser<'a> {
    pub(crate) fn parse_expr_for_test(&mut self) -> Expr {
        self.parse_expression()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_expr(src: &str) -> Expr {
        let tokens = crate::lex(src).expect("lex");
        let mut p = Parser::new(src, tokens);
        let e = p.parse_expr_for_test();
        assert!(p.diags.is_empty(), "{:?}", p.diags);
        e
    }

    #[test]
    fn bare_identifier() {
        match parse_expr("foo") {
            Expr::Ident(id) => assert_eq!(id.name, "foo"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn integer_vs_float_literal() {
        assert!(matches!(
            parse_expr("42"),
            Expr::Literal(Literal::Int { .. })
        ));
        assert!(matches!(
            parse_expr("3.5"),
            Expr::Literal(Literal::Float { .. })
        ));
        assert!(matches!(
            parse_expr("16rFF"),
            Expr::Literal(Literal::Int { .. })
        ));
    }

    #[test]
    fn nil_true_false_are_literals() {
        assert!(matches!(
            parse_expr("nil"),
            Expr::Literal(Literal::Nil { .. })
        ));
        assert!(matches!(
            parse_expr("true"),
            Expr::Literal(Literal::Bool { value: true, .. })
        ));
    }

    #[test]
    fn parenthesized() {
        assert!(matches!(parse_expr("(foo)"), Expr::Ident(_)));
    }
}
