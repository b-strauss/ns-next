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

/// How many steps the recovery (`docs/design/frontend.md`, "Recovery") may
/// take per token of the source: each token a speculative member parse or
/// a recovering re-parse consumes, each position rule 1's scan passes, each
/// free closer it looks up, and each member-start check the scan or a
/// recovery skip makes, with its lookahead. Each speculative member parse
/// can hold another, and the re-parse of a statement repeats the work of
/// the statements nested in it, so without a bound the work grows
/// exponentially with the nesting of half-typed members: 197 bytes nested
/// twelve deep took 0.6 s. Spent, the recovery stops looking for a member a
/// statement swallowed, and a skip stops only at a nested class or a lazy
/// slot: a file folds worse, never wrongly. The parse's work stays linear
/// in the source -- the budget's, one pass, and the speculative parses in
/// flight finishing. Ordinary edits spend little of it: twenty `)` removed
/// from `KernelTests.ns` spend 17,000 of 9.8 million, while a keyword
/// message of 8,000 parts holding an error spends it all, in 5 ms.
const RECOVERY_WORK_PER_TOKEN: usize = 256;

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
    /// How many more steps the recovery may take
    /// ([`RECOVERY_WORK_PER_TOKEN`]): spent by every token bumped while
    /// `recovering`, every position `member_holding_error` scans, every
    /// free closer `inside_block_or_tuple` looks up and every
    /// `recovery_at_member_start`. Never refilled, so the whole parse is
    /// bounded, not each statement.
    recovery_work_left: usize,
    /// How many speculative member parses, recovering re-parses and
    /// member-start checks of the recovery are running, nested. Raised and
    /// lowered around one call each, so it is zero between statements.
    recovering: u32,
    /// The free closers `inside_block_or_tuple` asks for, built the first
    /// time it asks, so a source without errors never builds them.
    free_closers: Option<declaration::FreeClosers>,
    /// The end `speculative_member_end` answered for a member, keyed by
    /// the offset it starts at and the bound the parse ran under. The
    /// answer depends on the depth it was asked at and on the budget left
    /// as well; those are kept out of the key by not keeping an answer the
    /// depth guard or a spent budget cut short, so every answer kept is
    /// the member's own. Reused at a depth or with a budget that would
    /// have refused, it is better than the refusal would have been, never
    /// wrong. Each statement of a run of half-typed members asks again for
    /// the members after it.
    member_ends: std::collections::HashMap<(u32, u32), Option<u32>>,
    /// How many times [`Parser::enter`] refused to go deeper: an answer
    /// the depth guard cut short depends on the depth it was asked at, so
    /// `member_ends` does not keep it.
    depth_refusals: usize,
    /// Every step the parse takes -- each token bumped, each position the
    /// recovery scans or checks for a member, each closer it looks up -- so
    /// a test can hold the parse's work to a bound without timing it.
    #[cfg(test)]
    steps: usize,
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str, tokens: Vec<Token>) -> Parser<'a> {
        let recovery_work_left = tokens.len().saturating_mul(RECOVERY_WORK_PER_TOKEN);
        Parser {
            source,
            tokens,
            position: 0,
            diagnostics: Vec::new(),
            depth: 0,
            bounds: Vec::new(),
            recovery_work_left,
            recovering: 0,
            free_closers: None,
            member_ends: std::collections::HashMap::new(),
            depth_refusals: 0,
            #[cfg(test)]
            steps: 0,
        }
    }

    /// Enter a guarded recursive production. Returns `false` once the nesting
    /// limit is exceeded; the caller must then push a diagnostic, recover, and
    /// return a placeholder node instead of recursing. Every call must be
    /// paired with exactly one [`Parser::leave`] on every return path (both the
    /// `true` and the `false` branch), so the count never leaks.
    fn enter(&mut self) -> bool {
        self.depth += 1;
        let allowed = self.depth <= MAX_DEPTH;
        if !allowed {
            self.depth_refusals += 1;
        }
        allowed
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
            self.count_step();
            if self.recovering > 0 {
                self.recovery_work_left = self.recovery_work_left.saturating_sub(1);
            }
        }
        token
    }

    /// A step of the parse's work, which only the tests count (`steps`).
    fn count_step(&mut self) {
        #[cfg(test)]
        {
            self.steps += 1;
        }
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
    /// or no name at all -- then nothing is consumed and there is none: a
    /// declaration without its name is left out, never given an empty one
    /// the resolver would call a duplicate.
    fn declared_identifier(&mut self, role: &str) -> Option<Identifier> {
        if !self.at_name() {
            let token = self.current();
            self.push_error(
                token.span,
                &format!("expected a name to {role}, found {}", self.found(token)),
            );
            return None;
        }
        let identifier = self.identifier();
        self.refuse_reserved_word(&identifier.name, identifier.span, role);
        Some(identifier)
    }

    fn push_error(&mut self, span: crate::span::Span, message: &str) {
        self.diagnostics
            .push(crate::diagnostic::Diagnostic::error(span, message));
    }

    /// The tokens consumed since `start`, as a type annotation; `None` when
    /// there are none.
    fn type_annotation_since(&self, start: usize) -> Option<TypeAnnotation> {
        let tokens = self.tokens.get(start..self.position)?;
        let (first, last) = (tokens.first()?, tokens.last()?);
        Some(TypeAnnotation {
            tokens: tokens
                .iter()
                .map(|token| TypeToken {
                    kind: token.kind,
                    text: self.text(*token).to_string(),
                })
                .collect(),
            span: first.span.merge(last.span),
        })
    }

    /// A method pattern's `where { ... }` type-constraint clause
    /// (`whereClause`, `NewspeakGrammar.ns:443`; the `<:` / `::` inference
    /// grammar of lines 382-434), kept as its tokens: the sub-grammar is large
    /// and only documents, so the clause is taken by balancing `{}` / `[]` /
    /// `()` until its closing brace. On an unterminated clause it pushes a
    /// diagnostic and stops at `EndOfInput` (parity with
    /// [`Parser::take_type_annotation`] / [`Parser::take_type_arguments`]).
    fn take_where_clause(&mut self) -> Option<TypeAnnotation> {
        let start = self.position;
        self.consume_where_clause();
        self.type_annotation_since(start)
    }

    fn consume_where_clause(&mut self) {
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

    /// A balanced `[...]` type-argument / type-parameter list
    /// (`typeArguments`, `NewspeakGrammar.ns:376`; `typeParameterDecl`,
    /// `:465`) — e.g. the `[E]` in `class List[E] ...` or `Collection[E] new`,
    /// or `[K, V]` in `Map[K, V]` -- kept as its tokens. Nested `[...]` are
    /// balanced; the scan stops at `EndOfInput`.
    fn take_type_arguments(&mut self) -> Option<TypeAnnotation> {
        let start = self.position;
        self.consume_type_arguments();
        self.type_annotation_since(start)
    }

    fn consume_type_arguments(&mut self) {
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

    /// A declaration-position `<Type>` annotation, kept as its tokens.
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
    /// The specification gives no type grammar, so the annotation is kept as
    /// the tokens written, not parsed further. Nested `<`, `[`, `{` and `(`
    /// groups are consumed until
    /// the `>` that closes the annotation, each closer matching the group it
    /// closes; the `<:` bound operator is one token, so it is not a nesting
    /// open. The scan never runs past a declaration boundary (`=`, `::=`, a
    /// closer the annotation did not open) or `EndOfInput`: on a malformed /
    /// unterminated `<...>` it pushes a diagnostic and returns without
    /// panicking. Every branch consumes a token or returns, so the loop
    /// always terminates.
    fn take_type_annotation(&mut self) -> Option<TypeAnnotation> {
        let start = self.position;
        self.consume_type_annotation();
        self.type_annotation_since(start)
    }

    fn consume_type_annotation(&mut self) {
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
                // falls through to the default arm below and is taken with
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

    /// The tokens as the parser ended with them (after it split `-4` and
    /// `>>`), and its diagnostics. Comments are attached against these
    /// tokens, so a reader that prints `-` and `4` apart counts as the
    /// attachment did.
    pub(crate) fn into_parts(self) -> (Vec<Token>, Vec<Diagnostic>) {
        (self.tokens, self.diagnostics)
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
        match parse_expression("foo").kind {
            ExpressionKind::Identifier(id) => assert_eq!(id.name, "foo"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn integer_versus_float_literal() {
        assert!(matches!(
            parse_expression("42").kind,
            ExpressionKind::Literal(Literal::Integer { .. })
        ));
        assert!(matches!(
            parse_expression("3.5").kind,
            ExpressionKind::Literal(Literal::Float { .. })
        ));
        assert!(matches!(
            parse_expression("16rFF").kind,
            ExpressionKind::Literal(Literal::Integer { .. })
        ));
    }

    #[test]
    fn nil_true_false_are_literals() {
        assert!(matches!(
            parse_expression("nil").kind,
            ExpressionKind::Literal(Literal::Nil { .. })
        ));
        assert!(matches!(
            parse_expression("true").kind,
            ExpressionKind::Literal(Literal::Boolean { value: true, .. })
        ));
    }

    #[test]
    fn parenthesized() {
        assert!(matches!(
            parse_expression("(foo)").kind,
            ExpressionKind::Identifier(_)
        ));
    }

    #[test]
    fn unary_chain() {
        // foo bar baz  ==  ((foo bar) baz)
        let expression = parse_expression("foo bar baz");
        let ExpressionKind::Send(send) = expression.kind else {
            panic!()
        };
        assert_eq!(send.message.selector, Selector::Unary("baz".into()));
        assert!(matches!(send.receiver.kind, ExpressionKind::Send(_)));
    }

    #[test]
    fn binary_left_associative_and_below_unary() {
        // a + b foo  ==  a + (b foo)
        let expression = parse_expression("a + b foo");
        let ExpressionKind::Send(send) = expression.kind else {
            panic!()
        };
        assert_eq!(send.message.selector, Selector::Binary("+".into()));
        assert!(matches!(
            send.message.arguments[0].kind,
            ExpressionKind::Send(_)
        ));
    }

    #[test]
    fn keyword_below_binary() {
        // d at: i + 1 put: x  ==  d at:(i+1) put:x  (one keyword send, selector at:put:)
        let expression = parse_expression("d at: i + 1 put: x");
        let ExpressionKind::Send(send) = expression.kind else {
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
        let ExpressionKind::Cascade(cascade) = expression.kind else {
            panic!()
        };
        assert_eq!(cascade.rest.len(), 2);
        assert_eq!(cascade.first.selector, Selector::Keyword("add:".into()));
    }

    #[test]
    fn setter_keyword_at_head_is_assign() {
        let expression = parse_expression("x:: 42");
        let ExpressionKind::Assign { target, value, .. } = expression.kind else {
            panic!("{expression:?}")
        };
        assert_eq!(target.name, "x");
        assert!(matches!(
            value.kind,
            ExpressionKind::Literal(Literal::Integer { .. })
        ));
    }

    #[test]
    fn recovers_from_bad_token() {
        let (expression, diagnostics) = parse_with_diagnostics(")");
        assert!(matches!(
            expression.kind,
            ExpressionKind::Literal(Literal::Nil { .. })
        ));
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn string_and_symbol_literals() {
        let ExpressionKind::Literal(Literal::String { value, .. }) = parse_expression("'hi'").kind
        else {
            panic!()
        };
        assert_eq!(value, "hi");
        let ExpressionKind::Literal(Literal::Symbol { value, .. }) = parse_expression("#foo").kind
        else {
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
        let ExpressionKind::Assign { target, value, .. } = expression.kind else {
            panic!("{expression:?}")
        };
        assert_eq!(target.name, "x");
        let ExpressionKind::Assign { target: inner, .. } = value.kind else {
            panic!()
        };
        assert_eq!(inner.name, "y");
    }

    #[test]
    fn block_with_parameters_and_temporaries() {
        let expression = parse_expression("[:x :y | | t | t:: x. t + y ]");
        let ExpressionKind::Block(block) = expression.kind else {
            panic!()
        };
        assert_eq!(
            block
                .parameters
                .iter()
                .map(|parameter| (
                    parameter.name.name.as_str(),
                    parameter.type_annotation.is_none()
                ))
                .collect::<Vec<_>>(),
            [("x", true), ("y", true)]
        );
        assert_eq!(block.body.temporaries.len(), 1);
        assert_eq!(block.body.statements.len(), 2);
    }

    #[test]
    fn empty_block() {
        let expression = parse_expression("[]");
        let ExpressionKind::Block(block) = expression.kind else {
            panic!()
        };
        assert!(block.parameters.is_empty() && block.body.statements.is_empty());
    }

    #[test]
    fn dynamic_array() {
        let expression = parse_expression("{ 1. 2 + 3. foo }");
        let ExpressionKind::DynamicArray { elements, .. } = expression.kind else {
            panic!()
        };
        assert_eq!(elements.len(), 3);
    }

    #[test]
    fn equals_and_relational_are_binary_sends() {
        // The lexer emits lone `=` / `<` / `>` as punctuation tokens; in
        // operand position they are binary selectors (grammar `binaryMsg`).
        let expression = parse_expression("a = b");
        let ExpressionKind::Send(send) = expression.kind else {
            panic!("{expression:?}")
        };
        assert_eq!(send.message.selector, Selector::Binary("=".into()));

        let expression = parse_expression("count < limit");
        let ExpressionKind::Send(send) = expression.kind else {
            panic!("{expression:?}")
        };
        assert_eq!(send.message.selector, Selector::Binary("<".into()));

        // `a = b ifTrue: [...]` == `(a = b) ifTrue: [...]`
        let expression = parse_expression("a = b ifTrue: [ c ]");
        let ExpressionKind::Send(send) = expression.kind else {
            panic!("{expression:?}")
        };
        assert_eq!(send.message.selector, Selector::Keyword("ifTrue:".into()));
        assert!(matches!(send.receiver.kind, ExpressionKind::Send(_)));
    }

    #[test]
    fn keyword_here_send_has_implicit_self_receiver() {
        // `selector: argument` with no explicit receiver — grammar
        // `keywordHereSend`.
        let expression = parse_expression("should: [ x ] description: 'oops'");
        let ExpressionKind::Send(send) = expression.kind else {
            panic!("{expression:?}")
        };
        assert_eq!(
            send.message.selector,
            Selector::Keyword("should:description:".into())
        );
        let ExpressionKind::Identifier(receiver) = &send.receiver.kind else {
            panic!("{:?}", send.receiver)
        };
        assert_eq!(receiver.name, "self");
    }

    #[test]
    fn return_statement_in_body() {
        // exercised via block body
        let expression = parse_expression("[ ^ 42 ]");
        let ExpressionKind::Block(block) = expression.kind else {
            panic!()
        };
        assert!(matches!(
            block.body.statements[0].kind,
            StatementKind::Return(_)
        ));
    }

    // --- Type annotations and other grammar corners ---

    #[test]
    fn typed_block_parameters() {
        let expression = parse_expression("[:x <Integer> :y <{A. B}> | x + y ]");
        let ExpressionKind::Block(block) = expression.kind else {
            panic!("{expression:?}")
        };
        assert_eq!(
            block
                .parameters
                .iter()
                .map(|parameter| (
                    parameter.name.name.as_str(),
                    parameter.type_annotation.as_ref().map(type_text)
                ))
                .collect::<Vec<_>>(),
            [
                ("x", Some("< Integer >".to_string())),
                ("y", Some("< { A . B } >".to_string()))
            ]
        );
    }

    #[test]
    fn class_is_a_pseudo_variable_and_a_unary_selector() {
        // `self class` — `class` as unary send.
        let ExpressionKind::Send(send) = parse_expression("self class").kind else {
            panic!()
        };
        assert_eq!(send.message.selector, Selector::Unary("class".into()));
        // `class onContext: it` — `class` as the receiver.
        let ExpressionKind::Send(send) = parse_expression("class onContext: it").kind else {
            panic!()
        };
        let ExpressionKind::Identifier(receiver) = &send.receiver.kind else {
            panic!()
        };
        assert_eq!(receiver.name, "class");
    }

    #[test]
    fn bar_is_a_binary_operator_in_operand_position() {
        // `a | b` and `a || b` are ordinary binary sends once a left operand
        // exists (the lexer emits `Bar` / `DoubleBar`).
        let ExpressionKind::Send(send) = parse_expression("a | b").kind else {
            panic!()
        };
        assert_eq!(send.message.selector, Selector::Binary("|".into()));
        let ExpressionKind::Send(send) = parse_expression("a || b").kind else {
            panic!()
        };
        assert_eq!(send.message.selector, Selector::Binary("||".into()));
    }

    #[test]
    fn an_eventual_send_runs_as_an_ordinary_one() {
        // `x <-: new` — async unary send, built as the ordinary send `x new`.
        let ExpressionKind::Send(send) = parse_expression("x <-: new").kind else {
            panic!("{:?}", parse_expression("x <-: new"))
        };
        assert_eq!(send.message.selector, Selector::Unary("new".into()));
        // `[ b ] <-: value fulfilled: [ r ]` — eventual send inside a keyword argument.
        let expression = parse_expression("Promise when: [ b ] <-: value fulfilled: [ r ]");
        let ExpressionKind::Send(send) = expression.kind else {
            panic!("{expression:?}")
        };
        assert_eq!(
            send.message.selector,
            Selector::Keyword("when:fulfilled:".into())
        );
    }

    #[test]
    fn literals_keep_their_spelling() {
        let source = "{'it''s'. \"a\". #foo. #'foo'. #at:put:. 16r1F}";
        let expression = parse_expression(source);
        let ExpressionKind::DynamicArray { elements, .. } = &expression.kind else {
            panic!("a tuple");
        };
        let texts: Vec<String> = elements
            .iter()
            .map(|element| match &element.kind {
                ExpressionKind::Literal(Literal::String { text, .. })
                | ExpressionKind::Literal(Literal::Symbol { text, .. })
                | ExpressionKind::Literal(Literal::Integer { text, .. }) => text.clone(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(
            texts,
            ["'it''s'", "\"a\"", "#foo", "#'foo'", "#at:put:", "16r1F"]
        );
    }

    #[test]
    fn an_eventual_send_is_marked() {
        let expression = parse_expression("a <-: foo: 3");
        let ExpressionKind::Send(send) = &expression.kind else {
            panic!("a send");
        };
        assert!(send.message.eventual);
        let ExpressionKind::Send(ordinary) = &parse_expression("a foo: 3").kind else {
            panic!("a send");
        };
        assert!(!ordinary.message.eventual);
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
        let ExpressionKind::Send(send) = parse_expression("bar: <at: 1>").kind else {
            panic!()
        };
        assert!(
            matches!(
                send.message.arguments[0].kind,
                ExpressionKind::Literal(Literal::Pattern { .. })
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
            let ExpressionKind::Literal(Literal::String { value, .. }) =
                parse_expression(source).kind
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
    fn generic_instantiation_keeps_type_arguments() {
        // `Array[E] new: n` — `[E]` type arguments on a primary.
        let expression = parse_expression("Array[E] new: 3");
        let ExpressionKind::Send(send) = expression.kind else {
            panic!("{expression:?}")
        };
        assert_eq!(send.message.selector, Selector::Keyword("new:".into()));
        let ExpressionKind::Identifier(receiver) = &send.receiver.kind else {
            panic!("{:?}", send.receiver)
        };
        assert_eq!(receiver.name, "Array");
        assert_eq!(
            type_text(send.receiver.type_arguments.as_ref().expect("[E]")),
            "[ E ]"
        );
    }

    #[test]
    fn parentheses_are_recorded_innermost_first() {
        let expression = parse_expression("((a foo)) bar");
        let ExpressionKind::Send(send) = &expression.kind else {
            panic!("a send");
        };
        assert!(expression.parentheses.is_empty());
        let receiver = &send.receiver;
        assert_eq!(receiver.parentheses.len(), 2);
        let source = "((a foo)) bar";
        assert_eq!(receiver.parentheses[0].delimiters.open.text(source), "(");
        assert_eq!(receiver.parentheses[0].delimiters.open.start, 1);
        assert_eq!(receiver.parentheses[1].delimiters.open.start, 0);
        assert_eq!(receiver.span.text(source), "((a foo))");
        assert!(matches!(receiver.kind, ExpressionKind::Send(_)));
    }

    #[test]
    fn the_implicit_receiver_is_marked_synthesized() {
        let expression = parse_expression("foo: 3");
        let ExpressionKind::Send(send) = &expression.kind else {
            panic!("a send");
        };
        assert!(send.receiver.synthesized);
        assert!(!expression.synthesized);
    }

    fn type_text(annotation: &TypeAnnotation) -> String {
        annotation
            .tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn type_arguments_after_a_unary_selector_are_kept() {
        let source = "a foo[E] bar";
        let expression = parse_expression(source);
        let ExpressionKind::Send(outer) = &expression.kind else {
            panic!("a send");
        };
        assert_eq!(outer.message.selector, Selector::Unary("bar".into()));
        assert!(outer.message.type_arguments.is_none());
        let ExpressionKind::Send(inner) = &outer.receiver.kind else {
            panic!("a foo[E]");
        };
        assert_eq!(inner.message.selector, Selector::Unary("foo".into()));
        let type_arguments = inner.message.type_arguments.as_ref().expect("[E]");
        assert_eq!(type_text(type_arguments), "[ E ]");
        assert_eq!(type_arguments.span.text(source), "[E]");
        assert!(outer.receiver.type_arguments.is_none());
    }

    #[test]
    fn type_arguments_after_a_parenthesis_are_kept() {
        let expression = parse_expression("(Box)[Integer] new");
        let ExpressionKind::Send(send) = &expression.kind else {
            panic!("a send");
        };
        let receiver = &send.receiver;
        assert!(receiver.type_arguments.is_none());
        assert_eq!(receiver.parentheses.len(), 1);
        assert_eq!(
            type_text(
                receiver.parentheses[0]
                    .type_arguments
                    .as_ref()
                    .expect("[Integer]")
            ),
            "[ Integer ]"
        );
        let expression = parse_expression("(Box[Integer]) new");
        let ExpressionKind::Send(send) = &expression.kind else {
            panic!("a send");
        };
        assert_eq!(
            type_text(send.receiver.type_arguments.as_ref().expect("[Integer]")),
            "[ Integer ]"
        );
        assert!(send.receiver.parentheses[0].type_arguments.is_none());
    }

    #[test]
    fn every_form_of_eventual_send_is_marked() {
        for (source, selector) in [
            ("a <-: foo", Selector::Unary("foo".into())),
            ("a <-: + 3", Selector::Binary("+".into())),
            ("a <-: at: 3", Selector::Keyword("at:".into())),
        ] {
            let expression = parse_expression(source);
            let ExpressionKind::Send(send) = &expression.kind else {
                panic!("{source}: a send");
            };
            assert_eq!(send.message.selector, selector, "{source}");
            assert!(send.message.eventual, "{source}");
        }
        for source in ["a foo", "a + 3", "a at: 3"] {
            let expression = parse_expression(source);
            let ExpressionKind::Send(send) = &expression.kind else {
                panic!("{source}: a send");
            };
            assert!(!send.message.eventual, "{source}");
        }
    }

    #[test]
    fn every_form_of_eventual_cascade_message_is_marked() {
        let expression = parse_expression("a foo; <-: bar; <-: + 3; <-: at: 3; baz");
        let ExpressionKind::Cascade(cascade) = &expression.kind else {
            panic!("a cascade");
        };
        assert!(!cascade.first.eventual);
        let marks: Vec<(Selector, bool)> = cascade
            .rest
            .iter()
            .map(|message| (message.selector.clone(), message.eventual))
            .collect();
        assert_eq!(
            marks,
            [
                (Selector::Unary("bar".into()), true),
                (Selector::Binary("+".into()), true),
                (Selector::Keyword("at:".into()), true),
                (Selector::Unary("baz".into()), false),
            ]
        );
    }

    #[test]
    fn more_literals_keep_their_spelling() {
        let ExpressionKind::Literal(Literal::Float { text, .. }) = parse_expression("1.5e3").kind
        else {
            panic!("a float");
        };
        assert_eq!(text, "1.5e3");
        for (source, expected) in [("#+", "#+"), ("#'a b'", "#'a b'")] {
            let ExpressionKind::Literal(Literal::Symbol { text, .. }) =
                parse_expression(source).kind
            else {
                panic!("{source}: a symbol");
            };
            assert_eq!(text, expected);
        }
    }

    #[test]
    fn a_split_negative_number_keeps_its_spelling() {
        // `3 -4` subtracts: the lexer's `-4` is split into the selector and
        // the number after it.
        for (source, argument_text) in [("3 -4", "4"), ("3 - -4", "-4")] {
            let expression = parse_expression(source);
            let ExpressionKind::Send(send) = &expression.kind else {
                panic!("{source}: a send");
            };
            assert_eq!(
                send.message.selector,
                Selector::Binary("-".into()),
                "{source}"
            );
            let ExpressionKind::Literal(Literal::Integer { text, span }) =
                &send.message.arguments[0].kind
            else {
                panic!("{source}: an integer argument");
            };
            assert_eq!(text, argument_text, "{source}");
            assert_eq!(span.text(source), argument_text, "{source}");
        }
    }
}
