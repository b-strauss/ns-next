use super::Parser;
use crate::ast::*;
use crate::token::TokenKind;

impl<'a> Parser<'a> {
    /// Recursion-depth-guarded entry to `primary`. Deeply nested input
    /// (`((((…`, `[[[[…`, `#((((…`, …) would otherwise recurse until the
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
            Expression::Literal(Literal::Nil { span: token.span })
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
                self.bump();
                let expression = self.parse_expression();
                self.expect(TokenKind::RightParenthesis);
                expression
            }
            TokenKind::LeftBracket => self.parse_block(),
            TokenKind::LeftBrace => self.parse_dynamic_array(),
            TokenKind::Number | TokenKind::String | TokenKind::Symbol => {
                Expression::Literal(self.parse_literal())
            }
            TokenKind::Identifier => {
                let token = self.current();
                match self.text(token) {
                    "nil" => {
                        self.bump();
                        Expression::Literal(Literal::Nil { span: token.span })
                    }
                    "true" => {
                        self.bump();
                        Expression::Literal(Literal::Boolean {
                            value: true,
                            span: token.span,
                        })
                    }
                    "false" => {
                        self.bump();
                        Expression::Literal(Literal::Boolean {
                            value: false,
                            span: token.span,
                        })
                    }
                    _ => Expression::Identifier(self.identifier()),
                }
            }
            // `class` lexes as the reserved `TokenKind::Class` token, but outside a
            // class-declaration prefix it is the pseudo-variable naming the
            // receiver's class (`self class`, `class onContext: it`). Grammar:
            // `primary = slotName | ...` (`NewspeakGrammar.ns:123`); the real
            // parser's `id` admits `class` in expression position.
            TokenKind::Class => Expression::Identifier(self.identifier()),
            TokenKind::LessThan => Expression::Literal(self.parse_pattern_literal()),
            TokenKind::Pound if self.peek2() == TokenKind::LeftParenthesis => {
                Expression::Literal(self.parse_literal_array())
            }
            _ => {
                let token = self.current();
                self.diagnostics.push(crate::diagnostic::Diagnostic::error(
                    token.span,
                    format!("expected an expression, found {:?}", token.kind),
                ));
                Expression::Literal(Literal::Nil { span: token.span })
            }
        }
    }

    fn parse_literal(&mut self) -> Literal {
        let token = self.bump();
        let raw = self.text(token).to_string();
        match token.kind {
            TokenKind::Number => {
                let is_radix = raw.contains('r');
                let looks_float = !is_radix && (raw.contains('.') || raw.contains('e'));
                if looks_float {
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
                span: token.span,
            },
            TokenKind::Symbol => Literal::Symbol {
                value: symbol_body(&raw),
                span: token.span,
            },
            _ => unreachable!("parse_literal called on {:?}", token.kind),
        }
    }

    // `<...>` — capture raw text including the angle brackets.
    fn parse_pattern_literal(&mut self) -> Literal {
        let start = self.bump(); // `<`
        let mut end = start;
        while !self.at(TokenKind::GreaterThan) && !self.at(TokenKind::EndOfInput) {
            end = self.bump();
        }
        if let Some(greater_than) = self.expect(TokenKind::GreaterThan) {
            end = greater_than;
        }
        let span = start.span.merge(end.span);
        Literal::Pattern {
            text: span.text(self.source).to_string(),
            span,
        }
    }

    fn parse_block(&mut self) -> Expression {
        let left_bracket = self
            .expect(TokenKind::LeftBracket)
            .unwrap_or(self.current());
        let mut parameters = Vec::new();
        while self.at(TokenKind::Colon) {
            self.bump();
            if self.at(TokenKind::Identifier) {
                parameters.push(self.declared_identifier("name a block parameter"));
                // `[:x <T> :y <T> | ...]` — block parameters carry the same
                // declaration-position `<Type>` annotations as method parameters.
                self.skip_type_annotation();
            }
        }
        if !parameters.is_empty() {
            self.expect(TokenKind::Bar);
        }
        let body = self.parse_code_body();
        let right_bracket = self
            .expect(TokenKind::RightBracket)
            .unwrap_or(self.current());
        let span = left_bracket.span.merge(right_bracket.span);
        Expression::Block(Box::new(BlockExpression {
            parameters,
            body,
            span,
        }))
    }

    fn parse_dynamic_array(&mut self) -> Expression {
        let left_brace = self.bump(); // {
        let mut elements = Vec::new();
        while !self.at(TokenKind::RightBrace) && !self.at(TokenKind::EndOfInput) {
            elements.push(self.parse_expression());
            if !self.eat(TokenKind::Dot) {
                break;
            }
        }
        let right_brace = self.expect(TokenKind::RightBrace).unwrap_or(self.current());
        Expression::DynamicArray {
            elements,
            span: left_brace.span.merge(right_brace.span),
        }
    }

    fn parse_literal_array(&mut self) -> Literal {
        let pound = self.bump(); // #
        self.expect(TokenKind::LeftParenthesis);
        let mut elements = Vec::new();
        while !self.at(TokenKind::RightParenthesis) && !self.at(TokenKind::EndOfInput) {
            elements.push(self.parse_array_element());
        }
        let right_parenthesis = self
            .expect(TokenKind::RightParenthesis)
            .unwrap_or(self.current());
        Literal::Array {
            elements,
            span: pound.span.merge(right_parenthesis.span),
        }
    }

    // Depth-guarded: nested `#( ( ( … ) ) )` literal arrays recurse here
    // without passing through `parse_primary`, so this mouth needs its own
    // guard sharing the same budget.
    fn parse_array_element(&mut self) -> Literal {
        let ok = self.enter();
        let literal = if ok {
            self.parse_array_element_inner()
        } else {
            let token = self.current();
            self.push_error(token.span, "expression nested too deeply");
            self.skip_to_structural();
            Literal::Nil { span: token.span }
        };
        self.leave();
        literal
    }

    fn parse_array_element_inner(&mut self) -> Literal {
        match self.peek() {
            TokenKind::LeftParenthesis => {
                let left_parenthesis = self.bump();
                let mut inner = Vec::new();
                while !self.at(TokenKind::RightParenthesis) && !self.at(TokenKind::EndOfInput) {
                    inner.push(self.parse_array_element());
                }
                let right_parenthesis = self
                    .expect(TokenKind::RightParenthesis)
                    .unwrap_or(self.current());
                Literal::Array {
                    elements: inner,
                    span: left_parenthesis.span.merge(right_parenthesis.span),
                }
            }
            TokenKind::Pound => {
                self.bump();
                self.parse_array_element()
            }
            TokenKind::Identifier => {
                let id = self.bump();
                // bare word inside a literal array is a symbol
                Literal::Symbol {
                    value: self.text(id).to_string(),
                    span: id.span,
                }
            }
            TokenKind::Number | TokenKind::String | TokenKind::Symbol => self.parse_literal(),
            _ => {
                let token = self.bump();
                self.diagnostics.push(crate::diagnostic::Diagnostic::error(
                    token.span,
                    "invalid literal-array element",
                ));
                Literal::Nil { span: token.span }
            }
        }
    }
}

impl<'a> Parser<'a> {
    pub(crate) fn parse_expression(&mut self) -> Expression {
        // `x:: v` at expression head — the grammar's `setterKeyword*
        // sendExpression`, §5.4.3's setter send and Newspeak's only assignment
        // form. Semantically a slot write, so represent it as Assign.
        // Setter-keyword *after* a receiver (`receiver x:: v`) is still a
        // keyword send, handled in parse_keyword_message.
        //
        // NOTE: the lexer emits `x::` as a single `SetterKeyword` token whose text
        // is `x::` (not `Identifier` followed by `SetterKeyword`), so the name is
        // split off the lexeme here rather than read as a separate token.
        if self.at(TokenKind::SetterKeyword) {
            let keyword = self.bump();
            let name = self.text(keyword).trim_end_matches(':');
            let target = Identifier {
                name: name.to_string(),
                span: crate::span::Span::new(
                    keyword.span.start,
                    keyword.span.start + name.len() as u32,
                ),
            };
            self.refuse_reserved_word(&target.name, target.span, "be assigned");
            let value = self.parse_expression();
            let span = target.span.merge(expression_span(&value));
            return Expression::Assign {
                target,
                value: Box::new(value),
                span,
            };
        }

        self.parse_cascade()
    }

    fn parse_cascade(&mut self) -> Expression {
        let first_full = self.parse_keyword_message();
        if !self.at(TokenKind::Semicolon) {
            return first_full;
        }
        // `first_full` must be a Send; its receiver is the cascade receiver.
        let Expression::Send(boxed) = first_full else {
            let token = self.current();
            self.diagnostics.push(crate::diagnostic::Diagnostic::error(
                token.span,
                "cascade must follow a message send",
            ));
            return first_full;
        };
        let Send {
            receiver,
            message,
            span: first_span,
        } = *boxed;
        let mut rest = Vec::new();
        while self.eat(TokenKind::Semicolon) {
            rest.push(self.parse_one_message_send());
        }
        let last = rest
            .last()
            .map(|message| message.span)
            .unwrap_or(first_span);
        Expression::Cascade(Box::new(Cascade {
            span: expression_span(&receiver).merge(last),
            receiver,
            first: message,
            rest,
        }))
    }

    fn parse_keyword_message(&mut self) -> Expression {
        // `keywordHereSend` (grammar `corpus/reference/NewspeakGrammar.ns:139`):
        // a keyword message with no explicit receiver (`foo: x bar: y`) is an
        // implicit send to `self`. Milestone 1 models the receiver as the
        // identifier `self` (zero-width span at the selector start).
        let mut receiver = if self.at(TokenKind::Keyword) {
            let selector_start = self.current().span.start;
            Expression::Identifier(Identifier {
                name: "self".to_string(),
                span: crate::span::Span::new(selector_start, selector_start),
            })
        } else {
            self.parse_binary_message()
        };
        if self.at(TokenKind::Keyword) || self.at(TokenKind::SetterKeyword) {
            let start = expression_span(&receiver);
            let mut selector = String::new();
            let mut arguments = Vec::new();
            let mut end = start;
            while self.at(TokenKind::Keyword) || self.at(TokenKind::SetterKeyword) {
                let keyword = self.bump();
                let keyword_text = self.text(keyword);
                // SetterKeyword lexeme is `foo::`; normalize to `foo:`
                let part = keyword_text.trim_end_matches(':');
                selector.push_str(part);
                selector.push(':');
                let argument = self.parse_binary_message();
                end = expression_span(&argument);
                arguments.push(argument);
            }
            receiver = Expression::Send(Box::new(Send {
                span: start.merge(end),
                receiver,
                message: MessageSend {
                    selector: Selector::Keyword(selector),
                    arguments,
                    span: start.merge(end),
                },
            }));
        }
        receiver
    }

    pub(super) fn parse_binary_message(&mut self) -> Expression {
        let mut receiver = self.parse_unary_message();
        // The lexer emits lone `=`, `<`, `>`, `|` and `||` as dedicated tokens
        // (`Equals` / `LessThan` / `GreaterThan` / `Bar` / `DoubleBar`) for
        // declaration and slot/temporary syntax, but in operand position — after a
        // primary — they are ordinary binary selectors per the grammar's
        // `binarySelector` (`corpus/reference/NewspeakGrammar.ns:126`,
        // `binaryMsg`). A leading `<` is still a pattern literal and a leading
        // `|` / `||` is still a temporary section (both handled before we get here),
        // so the ambiguity only bites where a left operand already exists. `|`
        // / `||` double as slot/temporary-section delimiters, so accept them as a
        // binary operator only when a primary can actually follow. A
        // section-closing `|` / `||` is followed by `RightParenthesis` / a declaration
        // keyword / `EndOfInput` / another `|` — none of which start a primary
        // — so a dot-less final slot initializer no longer eats its closing
        // delimiter and cascades into the following declaration. `= < >` and
        // plain `BinarySelector` are unconditional (their non-operator uses
        // cannot reach here with a left operand present).
        while matches!(
            self.peek(),
            TokenKind::BinarySelector
                | TokenKind::Equals
                | TokenKind::LessThan
                | TokenKind::GreaterThan
        ) || (matches!(self.peek(), TokenKind::Bar | TokenKind::DoubleBar)
            && starts_primary(self.peek2()))
        {
            let operator = self.bump();
            let argument = self.parse_unary_message();
            let span = expression_span(&receiver).merge(expression_span(&argument));
            receiver = Expression::Send(Box::new(Send {
                span,
                receiver,
                message: MessageSend {
                    selector: Selector::Binary(self.text(operator).to_string()),
                    arguments: vec![argument],
                    span,
                },
            }));
        }
        receiver
    }

    pub(super) fn parse_unary_message(&mut self) -> Expression {
        let mut receiver = self.parse_primary();
        // Generic instantiation `Array[E] new: n` — `[...]` type arguments on a
        // primary (`NewspeakGrammar.ns:376`). Dropped in Milestone 1.
        self.skip_type_arguments();
        loop {
            // Eventual-send operator `<-:` (`NewspeakGrammar.ns:50`) may prefix
            // any message. The lexer splits it into `BinarySelector("<-")` +
            // `Colon`; Milestone 1 drops the async marker and parses the
            // following message as an ordinary send.
            if self.at_eventual_send_operator() {
                self.bump(); // `<-`
                self.bump(); // `:`
            }
            if !(self.at(TokenKind::Identifier) || self.at(TokenKind::Class)) {
                break;
            }
            let selector_token = self.bump();
            let selector_name = self.text(selector_token).to_string();
            self.refuse_reserved_word(&selector_name, selector_token.span, "be a message selector");
            let span = expression_span(&receiver).merge(selector_token.span);
            receiver = Expression::Send(Box::new(Send {
                span,
                receiver,
                message: MessageSend {
                    selector: Selector::Unary(self.text(selector_token).to_string()),
                    arguments: Vec::new(),
                    span,
                },
            }));
            // `foo bar[E]` — type arguments on a unary send result.
            self.skip_type_arguments();
        }
        receiver
    }

    // A single `; message` element: unary | binary | keyword, no receiver.
    fn parse_one_message_send(&mut self) -> MessageSend {
        let start = self.current().span;
        match self.peek() {
            TokenKind::Keyword | TokenKind::SetterKeyword => {
                let mut selector = String::new();
                let mut arguments = Vec::new();
                let mut end = start;
                while self.at(TokenKind::Keyword) || self.at(TokenKind::SetterKeyword) {
                    let keyword = self.bump();
                    selector.push_str(self.text(keyword).trim_end_matches(':'));
                    selector.push(':');
                    let argument = self.parse_binary_message();
                    end = expression_span(&argument);
                    arguments.push(argument);
                }
                MessageSend {
                    selector: Selector::Keyword(selector),
                    arguments,
                    span: start.merge(end),
                }
            }
            TokenKind::BinarySelector => {
                let operator = self.bump();
                let argument = self.parse_unary_message();
                MessageSend {
                    selector: Selector::Binary(self.text(operator).to_string()),
                    span: start.merge(expression_span(&argument)),
                    arguments: vec![argument],
                }
            }
            TokenKind::Class => {
                let selector_token = self.bump();
                MessageSend {
                    selector: Selector::Unary(self.text(selector_token).to_string()),
                    arguments: Vec::new(),
                    span: selector_token.span,
                }
            }
            _ => {
                let selector_token = self.expect(TokenKind::Identifier).unwrap_or(self.current());
                let selector_name = self.text(selector_token).to_string();
                self.refuse_reserved_word(
                    &selector_name,
                    selector_token.span,
                    "be a message selector",
                );
                MessageSend {
                    selector: Selector::Unary(self.text(selector_token).to_string()),
                    arguments: Vec::new(),
                    span: selector_token.span,
                }
            }
        }
    }

    pub(super) fn is_reserved_identifier(&self, token: crate::token::Token) -> bool {
        matches!(self.text(token), "nil" | "true" | "false")
    }

    /// The eventual-send operator `<-:` (`NewspeakGrammar.ns:50`), which the
    /// lexer emits as `BinarySelector("<-")` followed by `Colon`.
    pub(super) fn at_eventual_send_operator(&self) -> bool {
        self.at(TokenKind::BinarySelector)
            && self.text(self.current()) == "<-"
            && self.peek2() == TokenKind::Colon
    }
}

/// Whether a token of this kind can begin a `primary` (`NewspeakGrammar.ns:123`
/// plus the parser's `nil`/`true`/`false`, `class` pseudo-variable and `<...>`
/// pattern-literal extensions). Used to disambiguate `|` / `||` as a binary
/// operator from a slot/temporary-section delimiter.
fn starts_primary(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Identifier
            | TokenKind::Class
            | TokenKind::Number
            | TokenKind::String
            | TokenKind::Symbol
            | TokenKind::LeftParenthesis
            | TokenKind::LeftBracket
            | TokenKind::LeftBrace
            | TokenKind::Pound
            | TokenKind::LessThan
    )
}

pub(super) fn expression_span(expression: &Expression) -> crate::span::Span {
    match expression {
        Expression::Identifier(identifier) => identifier.span,
        Expression::Literal(literal) => literal_span(literal),
        Expression::Send(send) => send.span,
        Expression::Cascade(cascade) => cascade.span,
        Expression::Assign { span, .. } => *span,
        Expression::Block(block) => block.span,
        Expression::DynamicArray { span, .. } => *span,
    }
}

pub(super) fn literal_span(literal: &Literal) -> crate::span::Span {
    match literal {
        Literal::Integer { span, .. }
        | Literal::Float { span, .. }
        | Literal::String { span, .. }
        | Literal::Symbol { span, .. }
        | Literal::Boolean { span, .. }
        | Literal::Nil { span }
        | Literal::Array { span, .. }
        | Literal::Pattern { span, .. } => *span,
    }
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
