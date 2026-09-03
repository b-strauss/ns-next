//! Recursive-descent + Pratt parser for Newspeak3.

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

    /// Consume and discard a method pattern's `where { ... }` type-constraint
    /// clause (`whereClause`, `NewspeakGrammar.ns:443`; the `<:` / `::`
    /// inference grammar of lines 382-434). This is a large sub-grammar with no
    /// Milestone 1 AST; the whole `where { ... }` is skipped by balancing
    /// `{}` / `[]` / `()` until the clause's closing brace. On an unterminated
    /// clause it pushes a diagnostic and stops at `Eoi` (parity with
    /// [`Parser::skip_type_annotation`] / [`Parser::skip_type_arguments`]).
    fn skip_where_clause(&mut self) {
        if !(self.at(TokenKind::Identifier)
            && self.text(self.cur()) == "where"
            && self.peek2() == TokenKind::LBrace)
        {
            return;
        }
        self.bump(); // `where`
        let open = self.bump(); // `{`
        let mut depth: u32 = 1;
        loop {
            match self.peek() {
                TokenKind::Eoi => {
                    self.push_error(open.span, "unterminated `where { ... }` clause");
                    return;
                }
                TokenKind::LBrace | TokenKind::LBracket | TokenKind::LParen => {
                    depth += 1;
                    self.bump();
                }
                TokenKind::RBrace | TokenKind::RBracket | TokenKind::RParen => {
                    self.bump();
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Consume and discard a balanced `[...]` type-argument / type-parameter
    /// list (`typeArguments`, `NewspeakGrammar.ns:376`; `typeParameterDecl`,
    /// `:465`) — e.g. the `[E]` in `class List[E] ...` or `Collection[E] new`,
    /// or `[K, V]` in `Map[K, V]`. Milestone 1 has no type AST, so the list is
    /// dropped. Nested `[...]` are balanced; the scan stops at `Eoi`.
    fn skip_type_arguments(&mut self) {
        if !self.at(TokenKind::LBracket) {
            return;
        }
        let open = self.bump(); // `[`
        let mut depth: u32 = 1;
        loop {
            match self.peek() {
                TokenKind::Eoi => {
                    self.push_error(open.span, "unterminated `[...]` type arguments");
                    return;
                }
                TokenKind::LBracket => {
                    depth += 1;
                    self.bump();
                }
                TokenKind::RBracket => {
                    self.bump();
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Consume and discard a declaration-position `<Type>` annotation.
    ///
    /// Typed Newspeak3 annotates declarations with `<Type>` — method/block
    /// parameters (`sel: p <Platform>`, `+ other <Foo>`), slot names
    /// (`public foo <Bar> = expr.`), and return positions. The vendored grammar
    /// wires these in via `typeArguments = lbracket , (typeExpr ...) , rbracket`
    /// (`corpus/reference/NewspeakGrammar.ns:376`) and the `<:` bound operator
    /// (`:49`, `:216`); type bodies therefore nest through `<...>` and `[...]`
    /// (`<Tuple[String]>`, `<Map[K, V]>`, `<Foo <: Bar>`).
    ///
    /// Milestone 1 has no AST type node, so — consistent with the
    /// `<T>`-as-`Literal::Pattern` stance in expression position
    /// (`parser::expr::parse_pattern_literal`) — the annotation is parsed and
    /// dropped here. Balanced `<`/`[` pairs are consumed until the matching
    /// `>`; a `<` immediately followed by `:` is the `<:` bound operator, not a
    /// nesting open. The scan never runs past a declaration boundary
    /// (`= ::= ) .` at depth 0) or `Eoi`: on a malformed / unterminated
    /// `<...>` it pushes a diagnostic and returns without panicking. Every
    /// branch consumes a token or returns, so the loop always terminates.
    fn skip_type_annotation(&mut self) {
        if !self.at(TokenKind::LessThan) {
            return;
        }
        let open = self.bump(); // `<`
        let mut depth: u32 = 1;
        loop {
            match self.peek() {
                TokenKind::Eoi => {
                    self.push_error(open.span, "unterminated `<...>` type annotation");
                    return;
                }
                TokenKind::LessThan => {
                    self.bump();
                    // `<:` is the mixin/bound operator, not a nesting open.
                    if !self.at(TokenKind::Colon) {
                        depth += 1;
                    }
                }
                TokenKind::LBracket | TokenKind::LBrace | TokenKind::LParen => {
                    depth += 1;
                    self.bump();
                }
                TokenKind::GreaterThan
                | TokenKind::RBracket
                | TokenKind::RBrace
                | TokenKind::RParen => {
                    self.bump();
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                // Safety net: a declaration boundary means the `<` never opened
                // an annotation — bail rather than eat the decl. `.` is a legal
                // tuple-type separator (`<{String. Namespace}>`) and `(` `)`
                // wrap arrow-type groups (`<[:(E | X) :E | X def]>`), so those
                // are not boundaries; a malformed `<...>` instead runs to `Eoi`.
                TokenKind::Equals | TokenKind::ColonColonEquals => {
                    self.push_error(open.span, "unterminated `<...>` type annotation");
                    return;
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Save the cursor position and the current diagnostics length, so a
    /// speculative parse can be rewound with [`Parser::restore`].
    pub(crate) fn checkpoint(&self) -> (usize, usize) {
        (self.pos, self.diags.len())
    }

    /// Rewind to a [`Parser::checkpoint`]: restore the cursor and drop any
    /// diagnostics pushed since (so speculative errors are discarded).
    pub(crate) fn restore(&mut self, cp: (usize, usize)) {
        self.pos = cp.0;
        self.diags.truncate(cp.1);
    }

    /// Consume the parser and return the diagnostics it accumulated.
    pub(crate) fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diags
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
    fn equals_and_relational_are_binary_sends() {
        // The lexer emits lone `=` / `<` / `>` as punctuation tokens; in
        // operand position they are binary selectors (grammar `binaryMsg`).
        let e = parse_expr("a = b");
        let Expr::Send(s) = e else { panic!("{e:?}") };
        assert_eq!(s.message.selector, Selector::Binary("=".into()));

        let e = parse_expr("count < limit");
        let Expr::Send(s) = e else { panic!("{e:?}") };
        assert_eq!(s.message.selector, Selector::Binary("<".into()));

        // `a = b ifTrue: [...]` == `(a = b) ifTrue: [...]`
        let e = parse_expr("a = b ifTrue: [ c ]");
        let Expr::Send(s) = e else { panic!("{e:?}") };
        assert_eq!(s.message.selector, Selector::Keyword("ifTrue:".into()));
        assert!(matches!(s.receiver, Expr::Send(_)));
    }

    #[test]
    fn keyword_here_send_has_implicit_self_receiver() {
        // `sel: arg` with no explicit receiver — grammar `keywordHereSend`.
        let e = parse_expr("should: [ x ] description: 'oops'");
        let Expr::Send(s) = e else { panic!("{e:?}") };
        assert_eq!(
            s.message.selector,
            Selector::Keyword("should:description:".into())
        );
        let Expr::Ident(recv) = &s.receiver else {
            panic!("{:?}", s.receiver)
        };
        assert_eq!(recv.name, "self");
    }

    #[test]
    fn return_statement_in_body() {
        // exercised via block body
        let e = parse_expr("[ ^ 42 ]");
        let Expr::Block(b) = e else { panic!() };
        assert!(matches!(b.body.stmts[0], Stmt::Return(_)));
    }

    // --- Task 14b ---

    #[test]
    fn typed_block_params() {
        let e = parse_expr("[:x <Integer> :y <{A. B}> | x + y ]");
        let Expr::Block(b) = e else { panic!("{e:?}") };
        assert_eq!(
            b.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["x", "y"]
        );
    }

    #[test]
    fn class_is_a_pseudo_variable_and_a_unary_selector() {
        // `self class` — `class` as unary send.
        let Expr::Send(s) = parse_expr("self class") else {
            panic!()
        };
        assert_eq!(s.message.selector, Selector::Unary("class".into()));
        // `class onContext: it` — `class` as the receiver.
        let Expr::Send(s) = parse_expr("class onContext: it") else {
            panic!()
        };
        let Expr::Ident(recv) = &s.receiver else {
            panic!()
        };
        assert_eq!(recv.name, "class");
    }

    #[test]
    fn bar_is_a_binary_operator_in_operand_position() {
        // `a | b` and `a || b` are ordinary binary sends once a left operand
        // exists (the lexer emits `Bar` / `DoubleBar`).
        let Expr::Send(s) = parse_expr("a | b") else {
            panic!()
        };
        assert_eq!(s.message.selector, Selector::Binary("|".into()));
        let Expr::Send(s) = parse_expr("a || b") else {
            panic!()
        };
        assert_eq!(s.message.selector, Selector::Binary("||".into()));
    }

    #[test]
    fn eventual_send_operator_is_dropped() {
        // `x <-: new` — async unary send; Milestone 1 parses it as `x new`.
        let Expr::Send(s) = parse_expr("x <-: new") else {
            panic!("{:?}", parse_expr("x <-: new"))
        };
        assert_eq!(s.message.selector, Selector::Unary("new".into()));
        // `[ b ] <-: value fulfilled: [ r ]` — eventual send inside a keyword arg.
        let e = parse_expr("Promise when: [ b ] <-: value fulfilled: [ r ]");
        let Expr::Send(s) = e else { panic!("{e:?}") };
        assert_eq!(
            s.message.selector,
            Selector::Keyword("when:fulfilled:".into())
        );
    }

    #[test]
    fn generic_instantiation_drops_type_arguments() {
        // `Array[E] new: n` — `[E]` type arguments on a primary.
        let e = parse_expr("Array[E] new: 3");
        let Expr::Send(s) = e else { panic!("{e:?}") };
        assert_eq!(s.message.selector, Selector::Keyword("new:".into()));
        let Expr::Ident(recv) = &s.receiver else {
            panic!("{:?}", s.receiver)
        };
        assert_eq!(recv.name, "Array");
    }
}
