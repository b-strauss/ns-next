//! Recursive-descent + Pratt parser for Newspeak3.

// Scaffold: the module is `mod parser;` (private) and, outside of tests, nothing
// in the crate constructs `Parser` yet — so in a non-test build the *entire*
// module reads as dead (every item is reachable only from `#[cfg(test)]` code).
// Per-item `#[allow(dead_code)]` would therefore have to annotate every item,
// which is strictly worse than one blanket allow. Task 12 adds the public entry
// point and a non-test caller; drop this allow then.
#![allow(dead_code)]

mod decl;
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

    // One-token lookahead, used by the message-send precedence ladder.
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

    fn push_error(&mut self, span: crate::span::Span, msg: &str) {
        self.diags
            .push(crate::diagnostic::Diagnostic::error(span, msg));
    }

    // `parse_expression` and the full precedence ladder live in `expr.rs`.
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

    // Sibling of `parse_expr` that does NOT assert a clean diagnostics list, for
    // exercising error recovery.
    fn parse_with_diags(src: &str) -> (Expr, Vec<Diagnostic>) {
        let tokens = crate::lex(src).expect("lex");
        let mut p = Parser::new(src, tokens);
        let e = p.parse_expr_for_test();
        (e, p.diags)
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

    #[test]
    fn unary_chain() {
        // foo bar baz  ==  ((foo bar) baz)
        let e = parse_expr("foo bar baz");
        let Expr::Send(s) = e else { panic!() };
        assert_eq!(s.message.selector, Selector::Unary("baz".into()));
        assert!(matches!(s.receiver, Expr::Send(_)));
    }

    #[test]
    fn binary_left_assoc_and_below_unary() {
        // a + b foo  ==  a + (b foo)
        let e = parse_expr("a + b foo");
        let Expr::Send(s) = e else { panic!() };
        assert_eq!(s.message.selector, Selector::Binary("+".into()));
        assert!(matches!(s.message.args[0], Expr::Send(_)));
    }

    #[test]
    fn keyword_below_binary() {
        // d at: i + 1 put: x  ==  d at:(i+1) put:x  (one keyword send, selector at:put:)
        let e = parse_expr("d at: i + 1 put: x");
        let Expr::Send(s) = e else { panic!() };
        assert_eq!(s.message.selector, Selector::Keyword("at:put:".into()));
        assert_eq!(s.message.args.len(), 2);
    }

    #[test]
    fn assignment_right_assoc() {
        let e = parse_expr("x := y := 1");
        let Expr::Assign { target, value, .. } = e else {
            panic!()
        };
        assert_eq!(target.name, "x");
        assert!(matches!(*value, Expr::Assign { .. }));
    }

    #[test]
    fn cascade() {
        let e = parse_expr("s add: 1; add: 2; yourself");
        let Expr::Cascade(c) = e else { panic!() };
        assert_eq!(c.rest.len(), 2);
        assert_eq!(c.first.selector, Selector::Keyword("add:".into()));
    }

    #[test]
    fn setter_keyword_at_head_is_assign() {
        let e = parse_expr("x:: 42");
        let Expr::Assign { target, value, .. } = e else {
            panic!("{e:?}")
        };
        assert_eq!(target.name, "x");
        assert!(matches!(*value, Expr::Literal(Literal::Int { .. })));
    }

    #[test]
    fn recovers_from_bad_token() {
        let (e, diags) = parse_with_diags(")");
        assert!(matches!(e, Expr::Literal(Literal::Nil { .. })));
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn string_and_symbol_literals() {
        let Expr::Literal(Literal::String { value, .. }) = parse_expr("'hi'") else {
            panic!()
        };
        assert_eq!(value, "hi");
        let Expr::Literal(Literal::Symbol { value, .. }) = parse_expr("#foo") else {
            panic!()
        };
        assert_eq!(value, "foo");
    }

    #[test]
    fn setter_keyword_after_receiver_is_keyword_send() {
        // `recv x:: v` stays a keyword send, not an assignment.
        let e = parse_expr("obj foo:: 5");
        let Expr::Send(s) = e else { panic!("{e:?}") };
        assert_eq!(s.message.selector, Selector::Keyword("foo:".into()));
    }

    #[test]
    fn nested_setter_keyword_at_head_is_nested_assign() {
        let e = parse_expr("x:: y:: 1");
        let Expr::Assign { target, value, .. } = e else {
            panic!("{e:?}")
        };
        assert_eq!(target.name, "x");
        let Expr::Assign { target: inner, .. } = *value else {
            panic!()
        };
        assert_eq!(inner.name, "y");
    }

    #[test]
    fn block_with_params_and_temps() {
        let e = parse_expr("[:x :y | | t | t := x. t + y ]");
        let Expr::Block(b) = e else { panic!() };
        assert_eq!(
            b.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["x", "y"]
        );
        assert_eq!(b.body.temps.len(), 1);
        assert_eq!(b.body.stmts.len(), 2);
    }

    #[test]
    fn empty_block() {
        let e = parse_expr("[]");
        let Expr::Block(b) = e else { panic!() };
        assert!(b.params.is_empty() && b.body.stmts.is_empty());
    }

    #[test]
    fn dynamic_array() {
        let e = parse_expr("{ 1. 2 + 3. foo }");
        let Expr::DynamicArray { elements, .. } = e else {
            panic!()
        };
        assert_eq!(elements.len(), 3);
    }

    #[test]
    fn literal_array() {
        let e = parse_expr("#(1 2 foo #bar (3 4))");
        let Expr::Literal(Literal::Array { elements, .. }) = e else {
            panic!()
        };
        assert_eq!(elements.len(), 5);
        assert!(matches!(elements[2], Literal::Symbol { .. })); // bare `foo`
        assert!(matches!(elements[4], Literal::Array { .. }));
    }

    #[test]
    fn return_statement_in_body() {
        // exercised via block body
        let e = parse_expr("[ ^ 42 ]");
        let Expr::Block(b) = e else { panic!() };
        assert!(matches!(b.body.stmts[0], Stmt::Return(_)));
    }
}
