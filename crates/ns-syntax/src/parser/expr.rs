use super::Parser;
use crate::ast::*;
use crate::token::TokenKind as T;

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
                T::RightParenthesis | T::RightBracket | T::RightBrace | T::Dot | T::EndOfInput => {
                    break
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn parse_primary_inner(&mut self) -> Expression {
        match self.peek() {
            T::LeftParenthesis => {
                self.bump();
                let expression = self.parse_expression();
                self.expect(T::RightParenthesis);
                expression
            }
            T::LeftBracket => self.parse_block(),
            T::LeftBrace => self.parse_dynamic_array(),
            T::Number | T::String | T::Symbol | T::CharacterConstant => {
                Expression::Literal(self.parse_literal())
            }
            T::Identifier => {
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
            // `class` lexes as the reserved `T::Class` token, but outside a
            // class-declaration prefix it is the pseudo-variable naming the
            // receiver's class (`self class`, `class onContext: it`). Grammar:
            // `primary = slotName | ...` (`NewspeakGrammar.ns:123`); the real
            // parser's `id` admits `class` in expression position.
            T::Class => Expression::Identifier(self.identifier()),
            T::LessThan => Expression::Literal(self.parse_pattern_literal()),
            T::Pound if self.peek2() == T::LeftParenthesis => {
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
            T::Number => {
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
            T::String => Literal::String {
                value: unescape_string(&raw),
                span: token.span,
            },
            T::Symbol => Literal::Symbol {
                value: symbol_body(&raw),
                span: token.span,
            },
            T::CharacterConstant => {
                let inner = super::lexeme_body(&raw);
                Literal::Character {
                    value: inner.chars().next().unwrap_or('\u{0}'),
                    span: token.span,
                }
            }
            _ => unreachable!("parse_literal called on {:?}", token.kind),
        }
    }

    // `<...>` — capture raw text including the angle brackets.
    fn parse_pattern_literal(&mut self) -> Literal {
        let start = self.bump(); // `<`
        let mut end = start;
        while !self.at(T::GreaterThan) && !self.at(T::EndOfInput) {
            end = self.bump();
        }
        if let Some(greater_than) = self.expect(T::GreaterThan) {
            end = greater_than;
        }
        let span = start.span.merge(end.span);
        Literal::Pattern {
            text: span.text(self.source).to_string(),
            span,
        }
    }

    fn parse_block(&mut self) -> Expression {
        let left_bracket = self.expect(T::LeftBracket).unwrap_or(self.current());
        let mut parameters = Vec::new();
        while self.at(T::Colon) {
            self.bump();
            if self.at(T::Identifier) {
                parameters.push(self.identifier());
                // `[:x <T> :y <T> | ...]` — block params carry the same
                // declaration-position `<Type>` annotations as method params.
                self.skip_type_annotation();
            }
        }
        if !parameters.is_empty() {
            self.expect(T::Bar);
        }
        let body = self.parse_code_body();
        let right_bracket = self.expect(T::RightBracket).unwrap_or(self.current());
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
        while !self.at(T::RightBrace) && !self.at(T::EndOfInput) {
            elements.push(self.parse_expression());
            if !self.eat(T::Dot) {
                break;
            }
        }
        let right_brace = self.expect(T::RightBrace).unwrap_or(self.current());
        Expression::DynamicArray {
            elements,
            span: left_brace.span.merge(right_brace.span),
        }
    }

    fn parse_literal_array(&mut self) -> Literal {
        let pound = self.bump(); // #
        self.expect(T::LeftParenthesis);
        let mut elements = Vec::new();
        while !self.at(T::RightParenthesis) && !self.at(T::EndOfInput) {
            elements.push(self.parse_array_element());
        }
        let right_parenthesis = self.expect(T::RightParenthesis).unwrap_or(self.current());
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
            T::LeftParenthesis => {
                let left_parenthesis = self.bump();
                let mut inner = Vec::new();
                while !self.at(T::RightParenthesis) && !self.at(T::EndOfInput) {
                    inner.push(self.parse_array_element());
                }
                let right_parenthesis = self.expect(T::RightParenthesis).unwrap_or(self.current());
                Literal::Array {
                    elements: inner,
                    span: left_parenthesis.span.merge(right_parenthesis.span),
                }
            }
            T::Pound => {
                self.bump();
                self.parse_array_element()
            }
            T::Identifier => {
                let id = self.bump();
                // bare word inside a literal array is a symbol
                Literal::Symbol {
                    value: self.text(id).to_string(),
                    span: id.span,
                }
            }
            T::Number | T::String | T::Symbol | T::CharacterConstant => self.parse_literal(),
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
        // assignment: Identifier ':=' expression
        if self.at(T::Identifier) && self.peek2() == T::Assign {
            let target = self.identifier();
            self.bump(); // :=
            let value = self.parse_expression();
            let span = target.span.merge(expression_span(&value));
            return Expression::Assign {
                target,
                value: Box::new(value),
                span,
            };
        }

        // `x:: v` at expression head — grammar's `setterKeyword*
        // sendExpression`. Semantically a slot write, so represent it as Assign
        // (== `x := v` for M1). Setter-keyword *after* a receiver (`receiver
        // x:: v`) is still a keyword send, handled in parse_keyword_message.
        //
        // NOTE: the lexer emits `x::` as a single `SetterKeyword` token whose text
        // is `x::` (not `Identifier` followed by `SetterKeyword`), so the name is
        // split off the lexeme here rather than read as a separate token.
        if self.at(T::SetterKeyword) {
            let keyword = self.bump();
            let name = self.text(keyword).trim_end_matches(':');
            let target = Identifier {
                name: name.to_string(),
                span: crate::span::Span::new(
                    keyword.span.start,
                    keyword.span.start + name.len() as u32,
                ),
            };
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
        if !self.at(T::Semicolon) {
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
        while self.eat(T::Semicolon) {
            rest.push(self.parse_one_message_send());
        }
        let last = rest
            .last()
            .map(|mutator| mutator.span)
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
        let mut receiver = if self.at(T::Keyword) {
            let selector_start = self.current().span.start;
            Expression::Identifier(Identifier {
                name: "self".to_string(),
                span: crate::span::Span::new(selector_start, selector_start),
            })
        } else {
            self.parse_binary_message()
        };
        if self.at(T::Keyword) || self.at(T::SetterKeyword) {
            let start = expression_span(&receiver);
            let mut selector = String::new();
            let mut arguments = Vec::new();
            let mut end = start;
            while self.at(T::Keyword) || self.at(T::SetterKeyword) {
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
        // declaration and slot/temp syntax, but in operand position — after a
        // primary — they are ordinary binary selectors per the grammar's
        // `binarySelector` (`corpus/reference/NewspeakGrammar.ns:126`,
        // `binaryMsg`). A leading `<` is still a pattern literal and a leading
        // `|` / `||` is still a temp section (both handled before we get here),
        // so the ambiguity only bites where a left operand already exists. `|`
        // / `||` double as slot/temp-section delimiters, so accept them as a
        // binary operator only when a primary can actually follow. A
        // section-closing `|` / `||` is followed by `RightParenthesis` / a decl
        // keyword / `EndOfInput` / another `|` — none of which start a primary
        // — so a dot-less final slot initializer no longer eats its closing
        // delimiter and cascades into the following declaration. `= < >` and
        // plain `BinarySelector` are unconditional (their non-operator uses
        // cannot reach here with a left operand present).
        while matches!(
            self.peek(),
            T::BinarySelector | T::Equals | T::LessThan | T::GreaterThan
        ) || (matches!(self.peek(), T::Bar | T::DoubleBar) && starts_primary(self.peek2()))
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
            if !((self.at(T::Identifier) || self.at(T::Class))
                && !self.is_reserved_identifier(self.current())
                && self.peek2() != T::Assign)
            {
                break;
            }
            let selector_token = self.bump();
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
            T::Keyword | T::SetterKeyword => {
                let mut selector = String::new();
                let mut arguments = Vec::new();
                let mut end = start;
                while self.at(T::Keyword) || self.at(T::SetterKeyword) {
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
            T::BinarySelector => {
                let operator = self.bump();
                let argument = self.parse_unary_message();
                MessageSend {
                    selector: Selector::Binary(self.text(operator).to_string()),
                    span: start.merge(expression_span(&argument)),
                    arguments: vec![argument],
                }
            }
            T::Class => {
                let selector_token = self.bump();
                MessageSend {
                    selector: Selector::Unary(self.text(selector_token).to_string()),
                    arguments: Vec::new(),
                    span: selector_token.span,
                }
            }
            _ => {
                let selector_token = self.expect(T::Identifier).unwrap_or(self.current());
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
        self.at(T::BinarySelector) && self.text(self.current()) == "<-" && self.peek2() == T::Colon
    }
}

/// Whether a token of this kind can begin a `primary` (`NewspeakGrammar.ns:123`
/// plus the parser's `nil`/`true`/`false`, `class` pseudo-variable and `<...>`
/// pattern-literal extensions). Used to disambiguate `|` / `||` as a binary
/// operator from a slot/temp-section delimiter.
fn starts_primary(kind: T) -> bool {
    matches!(
        kind,
        T::Identifier
            | T::Class
            | T::Number
            | T::String
            | T::Symbol
            | T::CharacterConstant
            | T::LeftParenthesis
            | T::LeftBracket
            | T::LeftBrace
            | T::Pound
            | T::LessThan
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
        | Literal::Character { span, .. }
        | Literal::Boolean { span, .. }
        | Literal::Nil { span }
        | Literal::Array { span, .. }
        | Literal::Pattern { span, .. } => *span,
    }
}

fn unescape_string(raw: &str) -> String {
    super::lexeme_body(raw).replace("''", "'")
}

fn symbol_body(raw: &str) -> String {
    let body = raw.strip_prefix('#').unwrap_or(raw);
    if body.starts_with('\'') {
        super::lexeme_body(body).replace("''", "'")
    } else {
        body.to_string()
    }
}
