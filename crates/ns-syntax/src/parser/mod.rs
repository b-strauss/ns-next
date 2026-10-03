//! Recursive-descent + Pratt parser for Newspeak3.

mod declaration;
mod expression;

use crate::ast::*;
use crate::diagnostic::Diagnostic;
use crate::token::{Token, TokenKind};

/// Maximum nesting depth for the recursive parser productions guarded by
/// [`Parser::enter`] / [`Parser::leave`] (`parse_primary`,
/// `parse_class_declaration`, a chain of setter keywords in `parse_expression`,
/// `parse_pattern_literal`). The deepest real corpus
/// program nests in the single digits; 128 is far past any genuine program
/// and well short of the (uncatchable) debug-build stack-overflow point at
/// roughly 250 frames.
const MAX_DEPTH: u32 = 128;

pub struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    position: usize,
    diagnostics: Vec<Diagnostic>,
    /// Current recursion depth across the guarded productions. Incremented by
    /// [`Parser::enter`], decremented by [`Parser::leave`].
    depth: u32,
    /// The member starts a recovering re-parse reads as the end of input
    /// (`parse_statement`), innermost last: a diagnostic there names the
    /// member, not an end of input the source does not have.
    bounds: Vec<crate::span::Span>,
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str, tokens: Vec<Token>) -> Parser<'a> {
        Parser {
            source,
            tokens,
            position: 0,
            diagnostics: Vec::new(),
            depth: 0,
            bounds: Vec::new(),
        }
    }

    /// Enter a guarded recursive production. Returns `false` once the nesting
    /// limit is exceeded; the caller must then push a diagnostic, recover, and
    /// return a placeholder node instead of recursing. Every call must be
    /// paired with exactly one [`Parser::leave`] on every return path (both the
    /// `true` and the `false` branch), so the count never leaks.
    fn enter(&mut self) -> bool {
        self.depth += 1;
        self.depth <= MAX_DEPTH
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn current(&self) -> Token {
        self.tokens[self.position.min(self.tokens.len() - 1)]
    }

    fn peek(&self) -> TokenKind {
        self.current().kind
    }

    // One-token lookahead, used by the message-send precedence ladder.
    fn peek2(&self) -> TokenKind {
        self.tokens
            .get(self.position + 1)
            .map_or(TokenKind::EndOfInput, |token| token.kind)
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.peek() == kind
    }

    /// Consumes the current token, never an end of input: neither the real
    /// one nor the bound a recovering re-parse sets (`parse_statement`), so
    /// nothing parsed under that bound reaches past it.
    fn bump(&mut self) -> Token {
        let token = self.current();
        if self.position < self.tokens.len() - 1 && token.kind != TokenKind::EndOfInput {
            self.position += 1;
        }
        token
    }

    /// The span of the last token consumed: where a node that recovery ended
    /// early ends, so its span never reaches an unrelated token after it.
    pub(super) fn previous_span(&self) -> crate::span::Span {
        match self.position.checked_sub(1) {
            Some(index) => self.tokens[index].span,
            None => self.current().span,
        }
    }

    /// What a diagnostic says it found at `token`.
    fn found(&self, token: Token) -> String {
        if token.kind == TokenKind::EndOfInput && self.bounds.contains(&token.span) {
            "the start of the next member".to_string()
        } else {
            format!("{:?}", token.kind)
        }
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// `eat`, answering the token consumed.
    fn eat_token(&mut self, kind: TokenKind) -> Option<Token> {
        if self.at(kind) {
            Some(self.bump())
        } else {
            None
        }
    }

    fn expect(&mut self, kind: TokenKind) -> Option<Token> {
        if self.at(kind) {
            Some(self.bump())
        } else {
            let token = self.current();
            self.diagnostics.push(Diagnostic::error(
                token.span,
                format!("expected {kind:?}, found {}", self.found(token)),
            ));
            None
        }
    }

    // Returns `&'a str`, not an elided lifetime: the slice borrows
    // `self.source: &'a str`, not `&self`, so a caller can write
    // `self.text(self.bump())` without a borrow-checker conflict.
    fn text(&self, token: Token) -> &'a str {
        token.span.text(self.source)
    }

    /// Whether the current token can be an `identifier` (§4.2's `id`).
    fn at_name(&self) -> bool {
        is_name(self.peek())
    }

    fn identifier(&mut self) -> Identifier {
        let token = self.bump();
        Identifier {
            name: self.text(token).to_string(),
            span: token.span,
        }
    }

    /// §4.1: `self`, `super`, `outer`, `true`, `false` and `nil` are
    /// reserved words, so a program may not declare or send one as an
    /// ordinary identifier. The lexer keeps them identifiers, as the canonical
    /// grammar does (`identifier = tokenFor: id`); the parser refuses them at
    /// every place an identifier is declared or sent, with a message naming
    /// that place (`docs/design/frontend.md`, "The six reserved words").
    /// `role` completes "cannot ...": "name a parameter", "be assigned".
    fn refuse_reserved_word(&mut self, name: &str, span: crate::span::Span, role: &str) {
        if RESERVED_WORDS.contains(&name) {
            self.push_error(
                span,
                &format!("`{name}` is a reserved word and cannot {role} (§4.1)"),
            );
        }
    }

    /// `identifier`, refused if it is a reserved word (`refuse_reserved_word`)
    /// or no name at all -- then nothing is consumed, and the name is empty.
    fn declared_identifier(&mut self, role: &str) -> Identifier {
        if !self.at_name() {
            let token = self.current();
            self.push_error(
                token.span,
                &format!("expected a name to {role}, found {}", self.found(token)),
            );
            return Identifier {
                name: String::new(),
                span: token.span,
            };
        }
        let identifier = self.identifier();
        self.refuse_reserved_word(&identifier.name, identifier.span, role);
        identifier
    }

    fn push_error(&mut self, span: crate::span::Span, message: &str) {
        self.diagnostics
            .push(crate::diagnostic::Diagnostic::error(span, message));
    }

    /// Consume and discard a method pattern's `where { ... }` type-constraint
    /// clause (`whereClause`, `NewspeakGrammar.ns:443`; the `<:` / `::`
    /// inference grammar of lines 382-434). This is a large sub-grammar with no
    /// Milestone 1 AST; the whole `where { ... }` is skipped by balancing
    /// `{}` / `[]` / `()` until the clause's closing brace. On an unterminated
    /// clause it pushes a diagnostic and stops at `EndOfInput` (parity with
    /// [`Parser::skip_type_annotation`] / [`Parser::skip_type_arguments`]).
    fn skip_where_clause(&mut self) {
        if !(self.at(TokenKind::Identifier)
            && self.text(self.current()) == "where"
            && self.peek2() == TokenKind::LeftBrace)
        {
            return;
        }
        self.bump(); // `where`
        let open = self.bump(); // `{`
        let mut depth: u32 = 1;
        loop {
            match self.peek() {
                TokenKind::EndOfInput => {
                    self.push_error(open.span, "unterminated `where { ... }` clause");
                    return;
                }
                TokenKind::LeftBrace | TokenKind::LeftBracket | TokenKind::LeftParenthesis => {
                    depth += 1;
                    self.bump();
                }
                TokenKind::RightBrace | TokenKind::RightBracket | TokenKind::RightParenthesis => {
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
    /// dropped. Nested `[...]` are balanced; the scan stops at `EndOfInput`.
    fn skip_type_arguments(&mut self) {
        if !self.at(TokenKind::LeftBracket) {
            return;
        }
        let open = self.bump(); // `[`
        let mut depth: u32 = 1;
        loop {
            match self.peek() {
                TokenKind::EndOfInput => {
                    self.push_error(open.span, "unterminated `[...]` type arguments");
                    return;
                }
                TokenKind::LeftBracket => {
                    depth += 1;
                    self.bump();
                }
                TokenKind::RightBracket => {
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
    /// parameters (`selector: platform <Platform>`, `+ other <Foo>`), slot
    /// names (`public foo <Bar> = expression.`), and return positions. The
    /// vendored grammar wires these in via `typeArguments = lbracket ,
    /// (typeExpr ...) , rbracket` (`corpus/reference/NewspeakGrammar.ns:376`)
    /// and the `<:` bound operator (`:49`, `:216`); type bodies therefore nest
    /// through `<...>` and `[...]` (`<Tuple[String]>`, `<Map[K, V]>`, `<Foo <:
    /// Bar>`).
    ///
    /// Milestone 1 has no AST type node, so — consistent with the
    /// `<T>`-as-`Literal::Pattern` stance in expression position
    /// (`parser::expression::parse_pattern_literal`) — the annotation is parsed and
    /// dropped here. Nested `<`, `[`, `{` and `(` groups are consumed until
    /// the `>` that closes the annotation, each closer matching the group it
    /// closes; the `<:` bound operator is one token, so it is not a nesting
    /// open. The scan never runs past a declaration boundary (`=`, `::=`, a
    /// closer the annotation did not open) or `EndOfInput`: on a malformed /
    /// unterminated `<...>` it pushes a diagnostic and returns without
    /// panicking. Every branch consumes a token or returns, so the loop
    /// always terminates.
    fn skip_type_annotation(&mut self) {
        if !self.at(TokenKind::LessThan) {
            return;
        }
        let open = self.bump(); // `<`
        let mut expected_closers = vec![TokenKind::GreaterThan];
        loop {
            match self.peek() {
                TokenKind::EndOfInput => {
                    self.push_error(open.span, "unterminated `<...>` type annotation");
                    return;
                }
                TokenKind::LessThan => {
                    expected_closers.push(TokenKind::GreaterThan);
                    self.bump();
                }
                // The bound operator `<:` inside a type body (`<Foo <: Bar>`)
                // lexes as one token, so it never opens a nesting level; it
                // falls through to the default arm below and is discarded with
                // the rest of the annotation.
                TokenKind::LeftBracket => {
                    expected_closers.push(TokenKind::RightBracket);
                    self.bump();
                }
                TokenKind::LeftBrace => {
                    expected_closers.push(TokenKind::RightBrace);
                    self.bump();
                }
                TokenKind::LeftParenthesis => {
                    expected_closers.push(TokenKind::RightParenthesis);
                    self.bump();
                }
                kind @ (TokenKind::GreaterThan
                | TokenKind::RightBracket
                | TokenKind::RightBrace
                | TokenKind::RightParenthesis) => {
                    // A closer the annotation did not open belongs to the
                    // declaration around it: `| a <Foo |` then `)` must leave
                    // the header's `)` to the header.
                    if expected_closers.last() != Some(&kind) {
                        self.push_error(open.span, "unterminated `<...>` type annotation");
                        return;
                    }
                    self.bump();
                    expected_closers.pop();
                    if expected_closers.is_empty() {
                        return;
                    }
                }
                // Safety net: a declaration boundary means the `<` never opened
                // an annotation — bail rather than eat the declaration. `.` is a legal
                // tuple-type separator (`<{String. Namespace}>`) and `(` `)`
                // wrap arrow-type groups (`<[:(E | X) :E | X def]>`), so those
                // are not boundaries; a malformed `<...>` instead runs to a
                // closer it did not open or to `EndOfInput`.
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
        (self.position, self.diagnostics.len())
    }

    /// Rewind to a [`Parser::checkpoint`]: restore the cursor and drop any
    /// diagnostics pushed since (so speculative errors are discarded).
    pub(crate) fn restore(&mut self, checkpoint: (usize, usize)) {
        self.position = checkpoint.0;
        self.diagnostics.truncate(checkpoint.1);
    }

    /// Consume the parser and return the diagnostics it accumulated.
    pub(crate) fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    // `parse_expression` and the full precedence ladder live in
    // `expression.rs`.
}

/// §4.1's reserved words, which no program may use as an ordinary identifier.
pub(super) const RESERVED_WORDS: [&str; 6] = ["self", "super", "outer", "true", "false", "nil"];

/// Whether a token of this kind is an `id` (§4.2). §4.1 reserves only its six
/// words, and the grammar is a PEG with no separate scanner, so `class`,
/// `public`, `protected` and `private` are ids like any other: the
/// productions that start with one (`classToken`, `accessModifier`) try it
/// first, and wherever the grammar takes an `identifier` -- a name declared,
/// read, written or sent -- it is a name. The lexer gives the four words
/// tokens of their own only so the declarations can find them; this is the
/// one place that says they are names too (`docs/design/frontend.md`, "Words
/// that are not reserved").
pub(super) fn is_name(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Identifier
            | TokenKind::Class
            | TokenKind::Public
            | TokenKind::Protected
            | TokenKind::Private
    )
}

/// Strip the first and last character from a lexeme — the quotes of a
/// single- or double-quoted string literal, the inner `'...'` of a quoted
/// symbol.
///
/// Panic-safe: a lexeme shorter than two characters is returned unchanged
/// rather than sliced through a byte range that could underflow or land in the
/// middle of a multi-byte UTF-8 sequence. Behaviour is identical to
/// `&raw[1..raw.len() - 1]` for every well-formed ASCII-delimited lexeme.
pub(super) fn lexeme_body(raw: &str) -> &str {
    let mut chars = raw.char_indices();
    match (chars.next(), chars.next_back()) {
        (Some((_, first)), Some((last_start, _))) => &raw[first.len_utf8()..last_start],
        _ => raw,
    }
}

#[cfg(test)]
impl<'a> Parser<'a> {
    pub(crate) fn parse_expression_for_test(&mut self) -> Expression {
        self.parse_expression()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_expression(source: &str) -> Expression {
        let tokens = crate::lex(source).expect("lex");
        let mut parser = Parser::new(source, tokens);
        let expression = parser.parse_expression_for_test();
        assert!(parser.diagnostics.is_empty(), "{:?}", parser.diagnostics);
        expression
    }

    // Sibling of `parse_expression` that does NOT assert a clean diagnostics
    // list, for exercising error recovery.
    fn parse_with_diagnostics(source: &str) -> (Expression, Vec<Diagnostic>) {
        let tokens = crate::lex(source).expect("lex");
        let mut parser = Parser::new(source, tokens);
        let expression = parser.parse_expression_for_test();
        (expression, parser.diagnostics)
    }

    #[test]
    fn bare_identifier() {
        match parse_expression("foo") {
            Expression::Identifier(id) => assert_eq!(id.name, "foo"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn integer_versus_float_literal() {
        assert!(matches!(
            parse_expression("42"),
            Expression::Literal(Literal::Integer { .. })
        ));
        assert!(matches!(
            parse_expression("3.5"),
            Expression::Literal(Literal::Float { .. })
        ));
        assert!(matches!(
            parse_expression("16rFF"),
            Expression::Literal(Literal::Integer { .. })
        ));
    }

    #[test]
    fn nil_true_false_are_literals() {
        assert!(matches!(
            parse_expression("nil"),
            Expression::Literal(Literal::Nil { .. })
        ));
        assert!(matches!(
            parse_expression("true"),
            Expression::Literal(Literal::Boolean { value: true, .. })
        ));
    }

    #[test]
    fn parenthesized() {
        assert!(matches!(
            parse_expression("(foo)"),
            Expression::Identifier(_)
        ));
    }

    #[test]
    fn unary_chain() {
        // foo bar baz  ==  ((foo bar) baz)
        let expression = parse_expression("foo bar baz");
        let Expression::Send(send) = expression else {
            panic!()
        };
        assert_eq!(send.message.selector, Selector::Unary("baz".into()));
        assert!(matches!(send.receiver, Expression::Send(_)));
    }

    #[test]
    fn binary_left_associative_and_below_unary() {
        // a + b foo  ==  a + (b foo)
        let expression = parse_expression("a + b foo");
        let Expression::Send(send) = expression else {
            panic!()
        };
        assert_eq!(send.message.selector, Selector::Binary("+".into()));
        assert!(matches!(send.message.arguments[0], Expression::Send(_)));
    }

    #[test]
    fn keyword_below_binary() {
        // d at: i + 1 put: x  ==  d at:(i+1) put:x  (one keyword send, selector at:put:)
        let expression = parse_expression("d at: i + 1 put: x");
        let Expression::Send(send) = expression else {
            panic!()
        };
        assert_eq!(send.message.selector, Selector::Keyword("at:put:".into()));
        assert_eq!(send.message.arguments.len(), 2);
    }

    #[test]
    fn a_module_spelling_colon_equals_does_not_parse() {
        let source = indoc::indoc! {"
            class C = () (
                public m = ( | t | t := 1. ^t )
            ) : ()
        "};
        let Err(diagnostics) = crate::parse(source) else {
            panic!("`:=` parsed");
        };
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .starts_with("`:=` is not Newspeak syntax")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn cascade() {
        let expression = parse_expression("s add: 1; add: 2; yourself");
        let Expression::Cascade(cascade) = expression else {
            panic!()
        };
        assert_eq!(cascade.rest.len(), 2);
        assert_eq!(cascade.first.selector, Selector::Keyword("add:".into()));
    }

    #[test]
    fn setter_keyword_at_head_is_assign() {
        let expression = parse_expression("x:: 42");
        let Expression::Assign { target, value, .. } = expression else {
            panic!("{expression:?}")
        };
        assert_eq!(target.name, "x");
        assert!(matches!(
            *value,
            Expression::Literal(Literal::Integer { .. })
        ));
    }

    #[test]
    fn recovers_from_bad_token() {
        let (expression, diagnostics) = parse_with_diagnostics(")");
        assert!(matches!(
            expression,
            Expression::Literal(Literal::Nil { .. })
        ));
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn string_and_symbol_literals() {
        let Expression::Literal(Literal::String { value, .. }) = parse_expression("'hi'") else {
            panic!()
        };
        assert_eq!(value, "hi");
        let Expression::Literal(Literal::Symbol { value, .. }) = parse_expression("#foo") else {
            panic!()
        };
        assert_eq!(value, "foo");
    }

    #[test]
    fn a_setter_keyword_after_a_receiver_is_refused() {
        // §5.4.3: `keywordMsg` takes a keyword, never a setter keyword, so
        // `receiver x:: v` is no message at all.
        let (_, diagnostics) = parse_with_diagnostics("obj foo:: 5");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("stands only at the start")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn nested_setter_keyword_at_head_is_nested_assign() {
        let expression = parse_expression("x:: y:: 1");
        let Expression::Assign { target, value, .. } = expression else {
            panic!("{expression:?}")
        };
        assert_eq!(target.name, "x");
        let Expression::Assign { target: inner, .. } = *value else {
            panic!()
        };
        assert_eq!(inner.name, "y");
    }

    #[test]
    fn block_with_parameters_and_temporaries() {
        let expression = parse_expression("[:x :y | | t | t:: x. t + y ]");
        let Expression::Block(block) = expression else {
            panic!()
        };
        assert_eq!(
            block
                .parameters
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect::<Vec<_>>(),
            ["x", "y"]
        );
        assert_eq!(block.body.temporaries.len(), 1);
        assert_eq!(block.body.statements.len(), 2);
    }

    #[test]
    fn empty_block() {
        let expression = parse_expression("[]");
        let Expression::Block(block) = expression else {
            panic!()
        };
        assert!(block.parameters.is_empty() && block.body.statements.is_empty());
    }

    #[test]
    fn dynamic_array() {
        let expression = parse_expression("{ 1. 2 + 3. foo }");
        let Expression::DynamicArray { elements, .. } = expression else {
            panic!()
        };
        assert_eq!(elements.len(), 3);
    }

    #[test]
    fn equals_and_relational_are_binary_sends() {
        // The lexer emits lone `=` / `<` / `>` as punctuation tokens; in
        // operand position they are binary selectors (grammar `binaryMsg`).
        let expression = parse_expression("a = b");
        let Expression::Send(send) = expression else {
            panic!("{expression:?}")
        };
        assert_eq!(send.message.selector, Selector::Binary("=".into()));

        let expression = parse_expression("count < limit");
        let Expression::Send(send) = expression else {
            panic!("{expression:?}")
        };
        assert_eq!(send.message.selector, Selector::Binary("<".into()));

        // `a = b ifTrue: [...]` == `(a = b) ifTrue: [...]`
        let expression = parse_expression("a = b ifTrue: [ c ]");
        let Expression::Send(send) = expression else {
            panic!("{expression:?}")
        };
        assert_eq!(send.message.selector, Selector::Keyword("ifTrue:".into()));
        assert!(matches!(send.receiver, Expression::Send(_)));
    }

    #[test]
    fn keyword_here_send_has_implicit_self_receiver() {
        // `selector: argument` with no explicit receiver — grammar
        // `keywordHereSend`.
        let expression = parse_expression("should: [ x ] description: 'oops'");
        let Expression::Send(send) = expression else {
            panic!("{expression:?}")
        };
        assert_eq!(
            send.message.selector,
            Selector::Keyword("should:description:".into())
        );
        let Expression::Identifier(receiver) = &send.receiver else {
            panic!("{:?}", send.receiver)
        };
        assert_eq!(receiver.name, "self");
    }

    #[test]
    fn return_statement_in_body() {
        // exercised via block body
        let expression = parse_expression("[ ^ 42 ]");
        let Expression::Block(block) = expression else {
            panic!()
        };
        assert!(matches!(block.body.statements[0], Statement::Return(_)));
    }

    // --- Type annotations and other grammar corners ---

    #[test]
    fn typed_block_parameters() {
        let expression = parse_expression("[:x <Integer> :y <{A. B}> | x + y ]");
        let Expression::Block(block) = expression else {
            panic!("{expression:?}")
        };
        assert_eq!(
            block
                .parameters
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect::<Vec<_>>(),
            ["x", "y"]
        );
    }

    #[test]
    fn class_is_a_pseudo_variable_and_a_unary_selector() {
        // `self class` — `class` as unary send.
        let Expression::Send(send) = parse_expression("self class") else {
            panic!()
        };
        assert_eq!(send.message.selector, Selector::Unary("class".into()));
        // `class onContext: it` — `class` as the receiver.
        let Expression::Send(send) = parse_expression("class onContext: it") else {
            panic!()
        };
        let Expression::Identifier(receiver) = &send.receiver else {
            panic!()
        };
        assert_eq!(receiver.name, "class");
    }

    #[test]
    fn bar_is_a_binary_operator_in_operand_position() {
        // `a | b` and `a || b` are ordinary binary sends once a left operand
        // exists (the lexer emits `Bar` / `DoubleBar`).
        let Expression::Send(send) = parse_expression("a | b") else {
            panic!()
        };
        assert_eq!(send.message.selector, Selector::Binary("|".into()));
        let Expression::Send(send) = parse_expression("a || b") else {
            panic!()
        };
        assert_eq!(send.message.selector, Selector::Binary("||".into()));
    }

    #[test]
    fn eventual_send_operator_is_dropped() {
        // `x <-: new` — async unary send; Milestone 1 parses it as `x new`.
        let Expression::Send(send) = parse_expression("x <-: new") else {
            panic!("{:?}", parse_expression("x <-: new"))
        };
        assert_eq!(send.message.selector, Selector::Unary("new".into()));
        // `[ b ] <-: value fulfilled: [ r ]` — eventual send inside a keyword argument.
        let expression = parse_expression("Promise when: [ b ] <-: value fulfilled: [ r ]");
        let Expression::Send(send) = expression else {
            panic!("{expression:?}")
        };
        assert_eq!(
            send.message.selector,
            Selector::Keyword("when:fulfilled:".into())
        );
    }

    #[test]
    fn lexeme_body_strips_first_and_last_character_safely() {
        // Well-formed and degenerate inputs, plus a multi-byte body — none panic.
        assert_eq!(lexeme_body(""), "");
        assert_eq!(lexeme_body("'"), "'");
        assert_eq!(lexeme_body("''"), "");
        assert_eq!(lexeme_body("'x'"), "x");
        assert_eq!(lexeme_body("#sym"), "sy");
        assert_eq!(lexeme_body("'é'"), "é");
    }

    #[test]
    fn pattern_literal_produces_a_pattern_node() {
        // `parse_pattern_literal` fires for a `<...>` in expression position.
        let Expression::Send(send) = parse_expression("bar: <at: 1>") else {
            panic!()
        };
        assert!(
            matches!(
                send.message.arguments[0],
                Expression::Literal(Literal::Pattern { .. })
            ),
            "{:?}",
            send.message.arguments[0]
        );
    }

    /// A double-quoted string is a string literal (§5.1.5), with no escape:
    /// a doubled quote inside one is two quotes, and a single quote is
    /// itself. The one-character form `"""` is the string `"`.
    #[test]
    fn a_double_quoted_string_produces_a_string_literal() {
        for (source, expected) in [
            ("\"a\"", "a"),
            ("\"abc\"", "abc"),
            ("\"'\"", "'"),
            ("\"\"\"", "\""),
            ("\"\"", ""),
            ("\"it''s\"", "it''s"),
            ("'it''s'", "it's"),
        ] {
            let Expression::Literal(Literal::String { value, .. }) = parse_expression(source)
            else {
                panic!("{source} is not a string literal")
            };
            assert_eq!(value, expected, "{source}");
        }
    }

    #[test]
    fn deeply_nested_parentheses_recover_without_overflow() {
        // Public `parse` must return diagnostics, not abort. Completes fast.
        let source = "(".repeat(10_000);
        let tokens = crate::lex(&source).expect("lex");
        let mut parser = Parser::new(&source, tokens);
        let _ = parser.parse_expression_for_test();
        assert!(!parser.diagnostics.is_empty());
        assert!(parser
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("nested too deeply")));
    }

    #[test]
    fn generic_instantiation_drops_type_arguments() {
        // `Array[E] new: n` — `[E]` type arguments on a primary.
        let expression = parse_expression("Array[E] new: 3");
        let Expression::Send(send) = expression else {
            panic!("{expression:?}")
        };
        assert_eq!(send.message.selector, Selector::Keyword("new:".into()));
        let Expression::Identifier(receiver) = &send.receiver else {
            panic!("{:?}", send.receiver)
        };
        assert_eq!(receiver.name, "Array");
    }
}
