use super::Parser;
use crate::ast::*;
use crate::token::TokenKind;

impl<'a> Parser<'a> {
    /// Recursion-depth-guarded entry to `primary`. Deeply nested input
    /// (`((((…`, `[[[[…`, …) would otherwise recurse until the
    /// process aborts with an uncatchable stack overflow, violating the
    /// "malformed source produces `Diagnostic`s" contract. On exceeding
    /// [`MAX_DEPTH`](super::MAX_DEPTH) we emit one diagnostic, skip forward to
    /// the nearest structural token, and return a `Literal::Nil` recovery node
    /// (matching the `_ =>` arm below).
    pub(super) fn parse_primary(&mut self) -> Expression {
        let ok = self.enter();
        let expression = if ok {
            self.parse_primary_inner()
        } else {
            let token = self.current();
            self.push_error(token.span, "expression nested too deeply");
            self.skip_to_structural();
            bare(ExpressionKind::Literal(Literal::Nil { span: token.span }))
        };
        self.leave();
        expression
    }

    /// Skip forward to the nearest structural token (`)` `]` `}` `.` or end of
    /// input) without consuming end-of-input. Used to bail out of an
    /// over-deep recursion onto a token the unwinding callers can resynchronise
    /// on.
    pub(super) fn skip_to_structural(&mut self) {
        loop {
            match self.peek() {
                TokenKind::RightParenthesis
                | TokenKind::RightBracket
                | TokenKind::RightBrace
                | TokenKind::Dot
                | TokenKind::EndOfInput => break,
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn parse_primary_inner(&mut self) -> Expression {
        match self.peek() {
            TokenKind::LeftParenthesis => {
                let open = self.bump().span;
                let before = self.position;
                let mut expression = self.parse_expression();
                let mut close = self.eat_token(TokenKind::RightParenthesis);
                if close.is_none() {
                    // An expression that consumed nothing has reported its
                    // token already: one diagnostic for one fault.
                    if self.position != before {
                        let token = self.current();
                        self.push_error(
                            token.span,
                            &format!(
                                "expected `)` to close the parenthesis, found {}",
                                self.found(token)
                            ),
                        );
                    }
                    // A parenthesis holds one expression and no `.`: a `.`
                    // means it was never closed, so the skip stops there and
                    // leaves an enclosing body's `)` alone.
                    self.skip_balanced(&[TokenKind::Dot]);
                    close = self.eat_token(TokenKind::RightParenthesis);
                }
                let close = close.map(|token| token.span);
                expression.span = open.merge(close.unwrap_or(expression.span));
                expression.parentheses.push(Parenthesis {
                    delimiters: Delimiters { open, close },
                    type_arguments: None,
                });
                expression
            }
            TokenKind::LeftBracket => self.parse_block(),
            TokenKind::LeftBrace => self.parse_dynamic_array(),
            TokenKind::Number | TokenKind::String | TokenKind::Symbol => {
                bare(ExpressionKind::Literal(self.parse_literal()))
            }
            TokenKind::Identifier => {
                let token = self.current();
                match self.text(token) {
                    "nil" => {
                        self.bump();
                        bare(ExpressionKind::Literal(Literal::Nil { span: token.span }))
                    }
                    "true" => {
                        self.bump();
                        bare(ExpressionKind::Literal(Literal::Boolean {
                            value: true,
                            span: token.span,
                        }))
                    }
                    "false" => {
                        self.bump();
                        bare(ExpressionKind::Literal(Literal::Boolean {
                            value: false,
                            span: token.span,
                        }))
                    }
                    "super" => {
                        let identifier = self.identifier();
                        // §5.10: "A super send consists of the reserved word
                        // super, followed by a message clause"; alone it
                        // denotes nothing.
                        if !self.at_message() {
                            self.push_error(
                                identifier.span,
                                "`super` is followed by a message (§5.10)",
                            );
                        }
                        bare(ExpressionKind::Identifier(identifier))
                    }
                    "outer" => {
                        let identifier = self.identifier();
                        // §5.9: "An outer send consists of the reserved word
                        // outer, an identifier N, and a message clause". The
                        // name is required; the message is not, because
                        // `outer N` alone is the enclosing object the send
                        // would go to, as in canonical Newspeak (decided
                        // with the user on 2026-09-27;
                        // `docs/design/frontend.md`).
                        if !self.at_name() {
                            self.push_error(
                                identifier.span,
                                "`outer` is followed by the name of an enclosing class (§5.9)",
                            );
                        }
                        bare(ExpressionKind::Identifier(identifier))
                    }
                    _ => bare(ExpressionKind::Identifier(self.identifier())),
                }
            }
            // `class` and the access modifiers have tokens of their own, but
            // in an expression they are names (`super::is_name`): `class
            // onContext: it`, a slot read `^public`.
            TokenKind::Class | TokenKind::Public | TokenKind::Protected | TokenKind::Private => {
                bare(ExpressionKind::Identifier(self.identifier()))
            }
            TokenKind::LessThan => {
                let literal = self.parse_pattern_literal();
                if let Literal::Pattern { span, .. } = &literal {
                    self.record_exclusion(|exclusions| exclusions.patterns.push(*span));
                }
                bare(ExpressionKind::Literal(literal))
            }
            _ => {
                let token = self.current();
                self.diagnostics.push(crate::diagnostic::Diagnostic::error(
                    token.span,
                    format!("expected an expression, found {}", self.found(token)),
                ));
                bare(ExpressionKind::Literal(Literal::Nil { span: token.span }))
            }
        }
    }

    fn parse_literal(&mut self) -> Literal {
        let token = self.bump();
        let raw = self.text(token).to_string();
        match token.kind {
            TokenKind::Number => {
                self.check_radix_number(&raw, token.span);
                // §5.1.1: "A number that includes neither a fraction nor an
                // exponent denotes an object that obeys the protocol of class
                // Integer"; the others are non-integral. A radix number's
                // extended digits are uppercase, so an `e` in one is its
                // exponent, as in a decimal one.
                let non_integral = raw.contains('.') || raw.contains('e');
                if non_integral {
                    Literal::Float {
                        text: raw,
                        span: token.span,
                    }
                } else {
                    Literal::Integer {
                        text: raw,
                        span: token.span,
                    }
                }
            }
            TokenKind::String => Literal::String {
                value: unescape_string(&raw),
                text: raw,
                span: token.span,
            },
            TokenKind::Symbol => Literal::Symbol {
                value: symbol_body(&raw),
                text: raw,
                span: token.span,
            },
            _ => unreachable!("parse_literal called on {:?}", token.kind),
        }
    }

    /// A radix number whose radix or digits denote no number: §4.2's grammar
    /// derives `radix → digits "r"` and `extendedDigits → (digit |
    /// uppercaseLetter)+` for any radix and any such digit, but §5.1.1
    /// gives a value only to digits below the radix. The canonical parser
    /// refuses the rest while it parses ("Malformed number: digit exceeds
    /// radix", `NewspeakParsing.ns`), and so does this one; a radix is
    /// between 2 and 36, the most that ten digits and 26 letters can spell.
    fn check_radix_number(&mut self, text: &str, span: crate::span::Span) {
        let Some((radix_text, rest)) = text.split_once('r') else {
            return;
        };
        let radix = match radix_text.parse::<u32>() {
            Ok(radix) if (2..=36).contains(&radix) => radix,
            _ => {
                self.push_error(
                    span,
                    &format!("the radix of `{text}` is not between 2 and 36 (§5.1.1)"),
                );
                return;
            }
        };
        let mantissa = rest.split('e').next().unwrap_or(rest);
        if let Some(digit) = mantissa
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .find(|character| character.to_digit(36).is_none_or(|value| value >= radix))
        {
            self.push_error(
                span,
                &format!("`{digit}` is not a digit of radix {radix} in `{text}` (§5.1.1)"),
            );
        }
    }

    /// §5.1.9's pattern literal, kept as its source text:
    ///
    /// ```text
    /// pattern → "<" patternLiteral ">"
    /// patternLiteral → wildcardPattern | literalPattern | keywordPattern
    /// wildcardPattern → "_"
    /// literalPattern → number | symbol | characterConstant | string | tuple
    /// keywordPattern → kwPatternPair+
    /// kwPatternPair → keyword [kwPatternValue]
    /// kwPatternValue → wildcardPattern | literalPattern | variablePattern | nestedPatternLiteral
    /// variablePattern → "?" id
    /// ```
    fn parse_pattern_literal(&mut self) -> Literal {
        if !self.enter() {
            self.leave();
            let token = self.current();
            self.push_error(token.span, "pattern nested too deeply");
            self.skip_to_structural();
            return Literal::Nil { span: token.span };
        }
        self.pattern_depth += 1;
        let literal = self.parse_pattern_literal_inner();
        self.pattern_depth -= 1;
        self.leave();
        literal
    }

    fn parse_pattern_literal_inner(&mut self) -> Literal {
        let start = self.bump(); // `<`
        if self.at_kind(TokenKind::Keyword) {
            while self.at_kind(TokenKind::Keyword) {
                self.bump();
                if !self.at_pattern_end() {
                    self.parse_pattern_value();
                }
            }
        } else if !self.parse_wildcard_or_literal_pattern() {
            self.push_error(
                self.current().span,
                "expected `_`, a literal or a keyword in a pattern (§5.1.9)",
            );
        }
        let end = self.expect_pattern_end().unwrap_or(start.span);
        let span = start.span.merge(end);
        Literal::Pattern {
            text: span.text(self.source).to_string(),
            span,
        }
    }

    /// `kwPatternValue`.
    fn parse_pattern_value(&mut self) {
        if self.at_kind(TokenKind::LessThan) {
            let _ = self.parse_pattern_literal();
            return;
        }
        // `variablePattern`: `?` against an identifier.
        if self.at_kind(TokenKind::BinarySelector)
            && self.text(self.current()) == "?"
            && self.peek2() == TokenKind::Identifier
            && self.tokens[self.position + 1].span.start == self.current().span.end
        {
            self.bump();
            self.bump();
            return;
        }
        if !self.parse_wildcard_or_literal_pattern() {
            self.push_error(
                self.current().span,
                "expected `_`, a literal, `?name` or a nested pattern (§5.1.9)",
            );
            if !self.at_pattern_end() && !self.at_kind(TokenKind::EndOfInput) {
                self.bump();
            }
        }
    }

    /// `wildcardPattern | literalPattern`, answering whether one was there.
    fn parse_wildcard_or_literal_pattern(&mut self) -> bool {
        match self.peek() {
            TokenKind::Identifier if self.text(self.current()) == "_" => {
                self.bump();
                true
            }
            TokenKind::Number | TokenKind::String | TokenKind::Symbol => {
                let _ = self.parse_literal();
                true
            }
            TokenKind::LeftBrace => {
                let _ = self.parse_dynamic_array();
                true
            }
            _ => false,
        }
    }

    /// Whether the cursor is at a pattern's closing `>`, which the lexer may
    /// have joined with a nested pattern's (`<a: <b: 1>>` ends in `>>`).
    fn at_pattern_end(&self) -> bool {
        self.at_kind(TokenKind::GreaterThan)
            || (self.at_kind(TokenKind::BinarySelector)
                && self.text(self.current()).bytes().all(|byte| byte == b'>'))
    }

    /// Consumes one `>`, splitting it off a run of them, and answers its span.
    fn expect_pattern_end(&mut self) -> Option<crate::span::Span> {
        if self.at_kind(TokenKind::GreaterThan) {
            return Some(self.bump().span);
        }
        if self.at_pattern_end() {
            let token = self.current();
            let start = token.span.start;
            let rest = crate::token::Token {
                kind: if token.span.end - start == 2 {
                    TokenKind::GreaterThan
                } else {
                    TokenKind::BinarySelector
                },
                span: crate::span::Span::new(start + 1, token.span.end),
            };
            self.tokens[self.position] = rest;
            return Some(crate::span::Span::new(start, start + 1));
        }
        self.expect(TokenKind::GreaterThan);
        None
    }

    fn parse_block(&mut self) -> Expression {
        let left_bracket = self
            .expect(TokenKind::LeftBracket)
            .unwrap_or(self.current());
        let mut parameters = Vec::new();
        // `blockParameters → blockParameter+ vbar`, `blockParameter → colon
        // slotDecl` (§5.1.8). A parameter without its name is left out, and
        // the `|` is still the parameters' end, not a temporaries section.
        let mut has_parameters = false;
        while self.at_kind(TokenKind::Colon) {
            self.bump();
            has_parameters = true;
            let name = self.declared_identifier("name a block parameter");
            // `[:x <T> :y <T> | ...]` — block parameters carry the same
            // declaration-position `<Type>` annotations as method parameters.
            let type_annotation = self.take_type_annotation();
            parameters.extend(name.map(|name| Parameter {
                name,
                type_annotation,
            }));
        }
        if has_parameters {
            self.expect(TokenKind::Bar);
        }
        let body = self.parse_code_body();
        let right_bracket = self.expect(TokenKind::RightBracket);
        let end = right_bracket.map_or_else(|| self.previous_span(), |token| token.span);
        let span = left_bracket.span.merge(end);
        bare(ExpressionKind::Block(Box::new(BlockExpression {
            parameters,
            body,
            delimiters: Some(Delimiters {
                open: left_bracket.span,
                close: right_bracket.map(|token| token.span),
            }),
            comments: Vec::new(),
            span,
        })))
    }

    fn parse_dynamic_array(&mut self) -> Expression {
        let left_brace = self.bump(); // {
        let mut elements = Vec::new();
        while !self.at_kind(TokenKind::RightBrace) && !self.at_body_close() {
            let before = self.position;
            elements.push(self.parse_expression());
            if self.eat(TokenKind::Dot) || self.at_kind(TokenKind::RightBrace) {
                continue;
            }
            // An element that consumed nothing has reported its token
            // already: one diagnostic for one fault.
            if self.position != before {
                let token = self.current();
                self.push_error(
                    token.span,
                    &format!(
                        "expected `.` between a tuple's elements, found {}",
                        self.found(token)
                    ),
                );
            }
            // The skip stops at the tuple's own `}`, which closes no group it
            // skipped, and at an enclosing body's close: the tuple keeps the
            // elements after the junk and never takes its owner's bracket.
            if !self.skip_balanced(&[TokenKind::Dot]) || !self.eat(TokenKind::Dot) {
                break;
            }
        }
        let right_brace = self.expect(TokenKind::RightBrace);
        let end = right_brace.map_or_else(|| self.previous_span(), |token| token.span);
        bare(ExpressionKind::DynamicArray {
            elements,
            span: left_brace.span.merge(end),
        })
    }
}

impl<'a> Parser<'a> {
    /// `expression → setterKeyword* sendExpression` (§5.4.3). The grammar
    /// writes `[setterKeyword]`, but the section's own rationale is "to
    /// enable chaining of setter sends, e.g., `w::x::y`", and the canonical
    /// grammar has `setterKeyword star`, so a chain is accepted. A setter
    /// keyword stands only here, at an expression's head.
    ///
    /// The lexer emits `x::` as a single `SetterKeyword` token whose text is
    /// `x::`, so the name is split off the lexeme here.
    pub(crate) fn parse_expression(&mut self) -> Expression {
        if self.at_kind(TokenKind::SetterKeyword) {
            if !self.enter() {
                self.leave();
                let token = self.current();
                self.push_error(token.span, "expression nested too deeply");
                self.skip_to_structural();
                return bare(ExpressionKind::Literal(Literal::Nil { span: token.span }));
            }
            let keyword = self.bump();
            let name = self.text(keyword).trim_end_matches(':');
            let target = Identifier {
                name: name.to_string(),
                span: crate::span::Span::from_offsets(
                    keyword.span.start as usize,
                    keyword.span.start as usize + name.len(),
                ),
            };
            self.refuse_reserved_word(&target.name, target.span, "be assigned");
            let value = self.parse_expression();
            self.leave();
            let span = target.span.merge(expression_span(&value));
            return bare(ExpressionKind::Assign {
                target,
                value: Box::new(value),
                span,
            });
        }
        self.parse_send_expression()
    }

    /// `sendExpression → implicitKeywordSend | cascadedMessageExpression`,
    /// where `cascadedMessageExpression → primary [msgCascade]` and
    /// `msgCascade → nonEmptyMessages cascadeMsg*` (§5.4.4): a cascade
    /// follows a primary and at least one message sent to it, never an
    /// implicit-receiver keyword send and never a primary alone.
    fn parse_send_expression(&mut self) -> Expression {
        if self.at_kind(TokenKind::Keyword) {
            // `implicitKeywordSend`: a keyword message with no explicit
            // receiver (`foo: x bar: y`) is an implicit send to `self`, whose
            // receiver is modelled as the identifier `self` with a zero-width
            // span at the selector's start.
            let selector_start = self.current().span.start;
            let mut receiver = bare(ExpressionKind::Identifier(Identifier {
                name: "self".to_string(),
                span: crate::span::Span::new(selector_start, selector_start),
            }));
            receiver.synthesized = true;
            let expression = self.parse_keyword_messages(receiver);
            if self.at_kind(TokenKind::Semicolon) {
                self.push_error(
                    self.current().span,
                    "a keyword send to the implicit receiver cannot be cascaded (§5.4.4)",
                );
                return self.parse_cascade_on(expression);
            }
            return expression;
        }
        let primary = self.parse_primary_operand();
        let (expression, sent) = self.parse_message_tails(primary);
        if !self.at_kind(TokenKind::Semicolon) {
            return expression;
        }
        if !sent {
            self.push_error(
                self.current().span,
                "a cascade follows a message sent to its receiver (§5.4.4)",
            );
        }
        self.parse_cascade_on(expression)
    }

    /// The `cascadeMsg*` after `first`, whose last message's receiver is the
    /// cascade's receiver.
    fn parse_cascade_on(&mut self, first: Expression) -> Expression {
        let ExpressionKind::Send(boxed) = first.kind else {
            while self.eat(TokenKind::Semicolon) {
                let _ = self.parse_cascade_message();
            }
            return first;
        };
        let first_span = boxed.span;
        let (receiver, message) = boxed.into_receiver_and_message();
        let mut rest = Vec::new();
        while self.eat(TokenKind::Semicolon) {
            rest.extend(self.parse_cascade_message());
        }
        let last = rest
            .last()
            .map(|message| message.span)
            .unwrap_or(first_span);
        bare(ExpressionKind::Cascade(Box::new(Cascade {
            span: expression_span(&receiver).merge(last),
            receiver,
            first: message,
            rest,
        })))
    }

    /// A primary and the type arguments of the canonical grammar's generic
    /// instantiation, `Array[E] new: n` (`NewspeakGrammar.ns:376`): after a
    /// `)` they are its parenthesis's, otherwise the primary's own.
    fn parse_primary_operand(&mut self) -> Expression {
        let mut primary = self.parse_primary();
        let type_arguments = self.take_type_arguments();
        match primary.parentheses.last_mut() {
            Some(outermost) => outermost.type_arguments = type_arguments,
            None => primary.type_arguments = type_arguments,
        }
        primary
    }

    /// `unarySelector* binaryMsg* [keywordMsg]` sent to `receiver`, and
    /// whether any message was.
    fn parse_message_tails(&mut self, receiver: Expression) -> (Expression, bool) {
        let before = self.position;
        let receiver = self.parse_unary_messages(receiver);
        let receiver = self.parse_binary_messages(receiver);
        let receiver = if self.at_keyword_message() {
            self.parse_keyword_messages(receiver)
        } else {
            receiver
        };
        self.refuse_setter_after_receiver();
        (receiver, self.position != before)
    }

    /// `binaryExpression → unaryExpression binaryMsg*`, a keyword message's
    /// argument.
    pub(super) fn parse_binary_message(&mut self) -> Expression {
        let receiver = self.parse_unary_message();
        self.parse_binary_messages(receiver)
    }

    /// `unaryExpression → primary unarySelector*`, a binary message's
    /// argument.
    pub(super) fn parse_unary_message(&mut self) -> Expression {
        let primary = self.parse_primary_operand();
        self.parse_unary_messages(primary)
    }

    fn parse_unary_messages(&mut self, mut receiver: Expression) -> Expression {
        loop {
            let eventual = self.eat_eventual_send_operator(|parser| parser.at_name());
            if !self.at_name() {
                return receiver;
            }
            let selector_token = self.bump();
            let selector_name = self.text(selector_token).to_string();
            self.refuse_reserved_word(&selector_name, selector_token.span, "be a message selector");
            // `foo bar[E]` — type arguments on a unary send result.
            let type_arguments = self.take_type_arguments();
            let span = expression_span(&receiver).merge(selector_token.span);
            receiver = bare(ExpressionKind::Send(Box::new(Send {
                span,
                receiver,
                message: MessageSend {
                    selector: Selector::Unary(selector_name),
                    arguments: Vec::new(),
                    eventual,
                    type_arguments,
                    comments: Vec::new(),
                    selector_spans: vec![selector_token.span],
                    span,
                },
            })));
        }
    }

    fn parse_binary_messages(&mut self, mut receiver: Expression) -> Expression {
        loop {
            let eventual = self.eat_eventual_send_operator(|parser| parser.at_binary_operator());
            if !self.at_binary_operator() {
                return receiver;
            }
            let (operator, operator_span, argument) = self.parse_binary_operator_and_argument();
            let span = expression_span(&receiver).merge(expression_span(&argument));
            receiver = bare(ExpressionKind::Send(Box::new(Send {
                span,
                receiver,
                message: MessageSend {
                    selector: Selector::Binary(operator),
                    arguments: vec![argument],
                    eventual,
                    type_arguments: None,
                    comments: Vec::new(),
                    selector_spans: vec![operator_span],
                    span,
                },
            })));
        }
    }

    /// `binarySelector unaryExpression`, the cursor at the selector.
    fn parse_binary_operator_and_argument(&mut self) -> (String, crate::span::Span, Expression) {
        if self.at_kind(TokenKind::Number) {
            self.split_negative_number();
        }
        let operator = self.bump();
        let argument = self.parse_unary_message();
        (self.text(operator).to_string(), operator.span, argument)
    }

    /// `keywordMsg → (keyword binaryExpression)+` sent to `receiver`.
    fn parse_keyword_messages(&mut self, receiver: Expression) -> Expression {
        let eventual = self.eat_eventual_send_operator(|parser| parser.at_kind(TokenKind::Keyword));
        let start = expression_span(&receiver);
        let (selector, selector_spans, arguments, end) = self.parse_keyword_parts(start);
        let span = start.merge(end);
        bare(ExpressionKind::Send(Box::new(Send {
            span,
            receiver,
            message: MessageSend {
                selector: Selector::Keyword(selector),
                arguments,
                eventual,
                type_arguments: None,
                comments: Vec::new(),
                selector_spans,
                span,
            },
        })))
    }

    /// The `(keyword binaryExpression)+` at the cursor: the selector, the
    /// span of each keyword token, the arguments, and the span of the last
    /// argument (`start` when there is none).
    fn parse_keyword_parts(
        &mut self,
        start: crate::span::Span,
    ) -> (
        String,
        Vec<crate::span::Span>,
        Vec<Expression>,
        crate::span::Span,
    ) {
        let mut selector = String::new();
        let mut selector_spans = Vec::new();
        let mut arguments = Vec::new();
        let mut end = start;
        while self.at_kind(TokenKind::Keyword) {
            let keyword = self.bump();
            selector_spans.push(keyword.span);
            selector.push_str(self.text(keyword).trim_end_matches(':'));
            selector.push(':');
            let argument = self.parse_binary_message();
            end = expression_span(&argument);
            arguments.push(argument);
        }
        (selector, selector_spans, arguments, end)
    }

    /// §5.4.3: a setter keyword begins an expression and is sent to the
    /// implicit receiver; after a receiver, `receiver x:: v` is no message
    /// at all -- `keywordMsg` takes `keyword`, never `setterKeyword`.
    fn refuse_setter_after_receiver(&mut self) {
        if self.at_kind(TokenKind::SetterKeyword) {
            let token = self.bump();
            self.push_error(
                token.span,
                "a setter send `name:: value` stands only at the start of an expression (§5.4.3)",
            );
            let _ = self.parse_binary_message();
        }
    }

    /// `cascadeMsg → semicolon (keywordMsg | binaryMsg | unarySelector)`,
    /// the cursor after the `;`. `None` when no message is there: a cascade
    /// message the source does not have is left out.
    fn parse_cascade_message(&mut self) -> Option<MessageSend> {
        let start = self.current().span;
        let eventual = self.eat_eventual_send_operator(|parser| {
            parser.at_kind(TokenKind::Keyword) || parser.at_binary_operator() || parser.at_name()
        });
        if self.at_kind(TokenKind::Keyword) {
            let (selector, selector_spans, arguments, end) = self.parse_keyword_parts(start);
            return Some(MessageSend {
                selector: Selector::Keyword(selector),
                arguments,
                eventual,
                type_arguments: None,
                comments: Vec::new(),
                selector_spans,
                span: start.merge(end),
            });
        }
        if self.at_binary_operator() {
            let (operator, operator_span, argument) = self.parse_binary_operator_and_argument();
            return Some(MessageSend {
                selector: Selector::Binary(operator),
                eventual,
                type_arguments: None,
                comments: Vec::new(),
                selector_spans: vec![operator_span],
                span: start.merge(expression_span(&argument)),
                arguments: vec![argument],
            });
        }
        if self.at_name() {
            let selector_token = self.bump();
            let selector_name = self.text(selector_token).to_string();
            self.refuse_reserved_word(&selector_name, selector_token.span, "be a message selector");
            return Some(MessageSend {
                selector: Selector::Unary(selector_name),
                arguments: Vec::new(),
                eventual,
                type_arguments: None,
                comments: Vec::new(),
                selector_spans: vec![selector_token.span],
                span: selector_token.span,
            });
        }
        let token = self.current();
        if token.kind == TokenKind::SetterKeyword {
            self.refuse_setter_after_receiver();
        } else {
            self.push_error(token.span, "expected a message after `;`");
        }
        None
    }

    /// Whether a message -- unary, binary or keyword, or `<-:` before one --
    /// starts at the cursor.
    fn at_message(&self) -> bool {
        self.at_name()
            || self.at_binary_operator()
            || self.at_kind(TokenKind::Keyword)
            || self.at_kind(TokenKind::EventualSendOperator)
    }

    fn at_keyword_message(&self) -> bool {
        self.at_kind(TokenKind::Keyword)
            || (self.at_kind(TokenKind::EventualSendOperator) && self.peek2() == TokenKind::Keyword)
    }

    /// Whether a binary message starts here: a binary selector with an
    /// operand after it. `=`, `<`, `>`, `|` and `||` have tokens of their own
    /// for the declaration syntax but are binary selectors too (§4.2's
    /// `specialCharacter`), and a negative number against its left operand
    /// is `-` and a number (§4.2's `binSel`, `3 -4` subtracts). Like the
    /// PEG's `binaryMsg*`, a selector with no operand after it is not taken:
    /// a slot section's closing `|` stays one.
    pub(super) fn at_binary_operator(&self) -> bool {
        match self.peek() {
            TokenKind::Number => self.text(self.current()).starts_with('-'),
            TokenKind::BinarySelector
            | TokenKind::Equals
            | TokenKind::LessThan
            | TokenKind::GreaterThan
            | TokenKind::Bar
            | TokenKind::DoubleBar => starts_primary(self.peek2()),
            _ => false,
        }
    }

    /// Splits a negative number at the cursor into its `-` and the number
    /// after it, for the place where the PEG reads the `-` as a binary
    /// selector (`Lexer::lex_number`).
    pub(super) fn split_negative_number(&mut self) {
        let token = self.current();
        let start = token.span.start;
        self.tokens[self.position] = crate::token::Token {
            kind: TokenKind::BinarySelector,
            span: crate::span::Span::new(start, start + 1),
        };
        self.tokens.insert(
            self.position + 1,
            crate::token::Token {
                kind: TokenKind::Number,
                span: crate::span::Span::new(start + 1, token.span.end),
            },
        );
    }

    /// §5.6's `<-:`, which stands before a message and nowhere else: consumed
    /// when the message after it is one `message_follows` accepts, left for
    /// the caller that parses its kind when it is another, and reported when
    /// no message follows at all. The send is parsed as an ordinary one: there
    /// are no actors to send it to (`docs/design/frontend.md`, "What is
    /// kept but ignored, and what a reprint drops"). Answers whether the
    /// operator was consumed for a message of this kind, which then records
    /// `eventual`.
    fn eat_eventual_send_operator(&mut self, message_follows: impl Fn(&Self) -> bool) -> bool {
        if !self.at_kind(TokenKind::EventualSendOperator) {
            return false;
        }
        let operator = self.bump();
        if message_follows(self) {
            return true;
        }
        if self.at_kind(TokenKind::Keyword) || self.at_binary_operator() || self.at_name() {
            self.position -= 1;
            return false;
        }
        self.push_error(operator.span, "expected a message after `<-:` (§5.6)");
        false
    }

    pub(super) fn is_reserved_identifier(&self, token: crate::token::Token) -> bool {
        matches!(self.text(token), "nil" | "true" | "false")
    }
}

/// Whether a token of this kind can begin a `primary` (§5.4.3): a name, a
/// literal -- `<` opening a pattern -- a block or a parenthesized
/// expression.
pub(super) fn starts_primary(kind: TokenKind) -> bool {
    super::is_name(kind)
        || matches!(
            kind,
            TokenKind::Number
                | TokenKind::String
                | TokenKind::Symbol
                | TokenKind::LeftParenthesis
                | TokenKind::LeftBracket
                | TokenKind::LeftBrace
                | TokenKind::LessThan
        )
}

pub(super) fn expression_span(expression: &Expression) -> crate::span::Span {
    expression.span
}

/// An expression of `kind` with nothing around it, spanning the shape.
pub(super) fn bare(kind: ExpressionKind) -> Expression {
    let span = kind.span();
    Expression::bare(kind, span)
}

/// A string literal's value: a single-quoted one un-doubles `''`, and a
/// double-quoted one has no escape at all (`lexer::lex_double_quoted_string`).
fn unescape_string(raw: &str) -> String {
    if raw.starts_with('"') {
        super::lexeme_body(raw).to_string()
    } else {
        super::lexeme_body(raw).replace("''", "'")
    }
}

fn symbol_body(raw: &str) -> String {
    let body = raw.strip_prefix('#').unwrap_or(raw);
    if body.starts_with('\'') {
        super::lexeme_body(body).replace("''", "'")
    } else {
        body.to_string()
    }
}
