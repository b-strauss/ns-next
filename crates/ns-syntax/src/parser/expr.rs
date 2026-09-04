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
    pub(super) fn parse_primary(&mut self) -> Expr {
        let ok = self.enter();
        let e = if ok {
            self.parse_primary_inner()
        } else {
            let t = self.cur();
            self.push_error(t.span, "expression nested too deeply");
            self.skip_to_structural();
            Expr::Literal(Literal::Nil { span: t.span })
        };
        self.leave();
        e
    }

    /// Skip forward to the nearest structural token (`)` `]` `}` `.` or end of
    /// input) without consuming end-of-input. Used to bail out of an
    /// over-deep recursion onto a token the unwinding callers can resynchronise
    /// on.
    pub(super) fn skip_to_structural(&mut self) {
        loop {
            match self.peek() {
                T::RParen | T::RBracket | T::RBrace | T::Dot | T::Eoi => break,
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn parse_primary_inner(&mut self) -> Expr {
        match self.peek() {
            T::LParen => {
                self.bump();
                let e = self.parse_expression();
                self.expect(T::RParen);
                e
            }
            T::LBracket => self.parse_block(),
            T::LBrace => self.parse_dynamic_array(),
            T::Number | T::String | T::Symbol | T::CharConstant => {
                Expr::Literal(self.parse_literal())
            }
            T::Identifier => {
                let t = self.cur();
                match self.text(t) {
                    "nil" => {
                        self.bump();
                        Expr::Literal(Literal::Nil { span: t.span })
                    }
                    "true" => {
                        self.bump();
                        Expr::Literal(Literal::Bool {
                            value: true,
                            span: t.span,
                        })
                    }
                    "false" => {
                        self.bump();
                        Expr::Literal(Literal::Bool {
                            value: false,
                            span: t.span,
                        })
                    }
                    _ => Expr::Ident(self.ident()),
                }
            }
            // `class` lexes as the reserved `T::Class` token, but outside a
            // class-declaration prefix it is the pseudo-variable naming the
            // receiver's class (`self class`, `class onContext: it`). Grammar:
            // `primary = slotName | ...` (`NewspeakGrammar.ns:123`); the real
            // parser's `id` admits `class` in expression position.
            T::Class => Expr::Ident(self.ident()),
            T::LessThan => Expr::Literal(self.parse_pattern_literal()),
            T::Pound if self.peek2() == T::LParen => Expr::Literal(self.parse_literal_array()),
            _ => {
                let t = self.cur();
                self.diags.push(crate::diagnostic::Diagnostic::error(
                    t.span,
                    format!("expected an expression, found {:?}", t.kind),
                ));
                Expr::Literal(Literal::Nil { span: t.span })
            }
        }
    }

    fn parse_literal(&mut self) -> Literal {
        let t = self.bump();
        let raw = self.text(t).to_string();
        match t.kind {
            T::Number => {
                let is_radix = raw.contains('r');
                let looks_float = !is_radix && (raw.contains('.') || raw.contains('e'));
                if looks_float {
                    Literal::Float {
                        text: raw,
                        span: t.span,
                    }
                } else {
                    Literal::Int {
                        text: raw,
                        span: t.span,
                    }
                }
            }
            T::String => Literal::String {
                value: unescape_string(&raw),
                span: t.span,
            },
            T::Symbol => Literal::Symbol {
                value: symbol_body(&raw),
                span: t.span,
            },
            T::CharConstant => {
                let inner = super::lexeme_body(&raw);
                Literal::Char {
                    value: inner.chars().next().unwrap_or('\u{0}'),
                    span: t.span,
                }
            }
            _ => unreachable!("parse_literal called on {:?}", t.kind),
        }
    }

    // `<...>` — capture raw text including the angle brackets.
    fn parse_pattern_literal(&mut self) -> Literal {
        let start = self.bump(); // `<`
        let mut end = start;
        while !self.at(T::GreaterThan) && !self.at(T::Eoi) {
            end = self.bump();
        }
        if let Some(gt) = self.expect(T::GreaterThan) {
            end = gt;
        }
        let span = start.span.merge(end.span);
        Literal::Pattern {
            text: span.text(self.src).to_string(),
            span,
        }
    }

    fn parse_block(&mut self) -> Expr {
        let lb = self.expect(T::LBracket).unwrap_or(self.cur());
        let mut params = Vec::new();
        while self.at(T::Colon) {
            self.bump();
            if self.at(T::Identifier) {
                params.push(self.ident());
                // `[:x <T> :y <T> | ...]` — block params carry the same
                // declaration-position `<Type>` annotations as method params.
                self.skip_type_annotation();
            }
        }
        if !params.is_empty() {
            self.expect(T::Bar);
        }
        let body = self.parse_code_body();
        let rb = self.expect(T::RBracket).unwrap_or(self.cur());
        let span = lb.span.merge(rb.span);
        Expr::Block(Box::new(BlockExpr { params, body, span }))
    }

    fn parse_dynamic_array(&mut self) -> Expr {
        let lb = self.bump(); // {
        let mut elements = Vec::new();
        while !self.at(T::RBrace) && !self.at(T::Eoi) {
            elements.push(self.parse_expression());
            if !self.eat(T::Dot) {
                break;
            }
        }
        let rb = self.expect(T::RBrace).unwrap_or(self.cur());
        Expr::DynamicArray {
            elements,
            span: lb.span.merge(rb.span),
        }
    }

    fn parse_literal_array(&mut self) -> Literal {
        let pound = self.bump(); // #
        self.expect(T::LParen);
        let mut elements = Vec::new();
        while !self.at(T::RParen) && !self.at(T::Eoi) {
            elements.push(self.parse_array_element());
        }
        let rp = self.expect(T::RParen).unwrap_or(self.cur());
        Literal::Array {
            elements,
            span: pound.span.merge(rp.span),
        }
    }

    // Depth-guarded: nested `#( ( ( … ) ) )` literal arrays recurse here
    // without passing through `parse_primary`, so this mouth needs its own
    // guard sharing the same budget.
    fn parse_array_element(&mut self) -> Literal {
        let ok = self.enter();
        let lit = if ok {
            self.parse_array_element_inner()
        } else {
            let t = self.cur();
            self.push_error(t.span, "expression nested too deeply");
            self.skip_to_structural();
            Literal::Nil { span: t.span }
        };
        self.leave();
        lit
    }

    fn parse_array_element_inner(&mut self) -> Literal {
        match self.peek() {
            T::LParen => {
                let lp = self.bump();
                let mut inner = Vec::new();
                while !self.at(T::RParen) && !self.at(T::Eoi) {
                    inner.push(self.parse_array_element());
                }
                let rp = self.expect(T::RParen).unwrap_or(self.cur());
                Literal::Array {
                    elements: inner,
                    span: lp.span.merge(rp.span),
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
            T::Number | T::String | T::Symbol | T::CharConstant => self.parse_literal(),
            _ => {
                let t = self.bump();
                self.diags.push(crate::diagnostic::Diagnostic::error(
                    t.span,
                    "invalid literal-array element",
                ));
                Literal::Nil { span: t.span }
            }
        }
    }
}

impl<'a> Parser<'a> {
    pub(crate) fn parse_expression(&mut self) -> Expr {
        // assignment: Ident ':=' expression
        if self.at(T::Identifier) && self.peek2() == T::Assign {
            let target = self.ident();
            self.bump(); // :=
            let value = self.parse_expression();
            let span = target.span.merge(expr_span(&value));
            return Expr::Assign {
                target,
                value: Box::new(value),
                span,
            };
        }

        // `x:: v` at expression head — grammar's `setterKeyword* sendExpression`.
        // Semantically a slot write, so represent it as Assign (== `x := v` for M1).
        // Setter-keyword *after* a receiver (`recv x:: v`) is still a keyword send,
        // handled in parse_keyword_message.
        //
        // NOTE: the lexer emits `x::` as a single `SetterKeyword` token whose text
        // is `x::` (not `Identifier` followed by `SetterKeyword`), so the name is
        // split off the lexeme here rather than read as a separate token.
        if self.at(T::SetterKeyword) {
            let kw = self.bump();
            let name = self.text(kw).trim_end_matches(':');
            let target = Ident {
                name: name.to_string(),
                span: crate::span::Span::new(kw.span.start, kw.span.start + name.len() as u32),
            };
            let value = self.parse_expression();
            let span = target.span.merge(expr_span(&value));
            return Expr::Assign {
                target,
                value: Box::new(value),
                span,
            };
        }

        self.parse_cascade()
    }

    fn parse_cascade(&mut self) -> Expr {
        let first_full = self.parse_keyword_message();
        if !self.at(T::Semicolon) {
            return first_full;
        }
        // `first_full` must be a Send; its receiver is the cascade receiver.
        let Expr::Send(boxed) = first_full else {
            let t = self.cur();
            self.diags.push(crate::diagnostic::Diagnostic::error(
                t.span,
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
        let last = rest.last().map(|m| m.span).unwrap_or(first_span);
        Expr::Cascade(Box::new(Cascade {
            span: expr_span(&receiver).merge(last),
            receiver,
            first: message,
            rest,
        }))
    }

    fn parse_keyword_message(&mut self) -> Expr {
        // `keywordHereSend` (grammar `corpus/reference/NewspeakGrammar.ns:139`):
        // a keyword message with no explicit receiver (`foo: x bar: y`) is an
        // implicit send to `self`. Milestone 1 models the receiver as the
        // identifier `self` (zero-width span at the selector start).
        let mut recv = if self.at(T::Keyword) {
            let at = self.cur().span.start;
            Expr::Ident(Ident {
                name: "self".to_string(),
                span: crate::span::Span::new(at, at),
            })
        } else {
            self.parse_binary_message()
        };
        if self.at(T::Keyword) || self.at(T::SetterKeyword) {
            let start = expr_span(&recv);
            let mut selector = String::new();
            let mut args = Vec::new();
            let mut end = start;
            while self.at(T::Keyword) || self.at(T::SetterKeyword) {
                let kw = self.bump();
                let kw_text = self.text(kw);
                // SetterKeyword lexeme is `foo::`; normalize to `foo:`
                let part = kw_text.trim_end_matches(':');
                selector.push_str(part);
                selector.push(':');
                let arg = self.parse_binary_message();
                end = expr_span(&arg);
                args.push(arg);
            }
            recv = Expr::Send(Box::new(Send {
                span: start.merge(end),
                receiver: recv,
                message: MessageSend {
                    selector: Selector::Keyword(selector),
                    args,
                    span: start.merge(end),
                },
            }));
        }
        recv
    }

    pub(super) fn parse_binary_message(&mut self) -> Expr {
        let mut recv = self.parse_unary_message();
        // The lexer emits lone `=`, `<`, `>`, `|` and `||` as dedicated tokens
        // (`Equals` / `LessThan` / `GreaterThan` / `Bar` / `DoubleBar`) for
        // declaration and slot/temp syntax, but in operand position — after a
        // primary — they are ordinary binary selectors per the grammar's
        // `binarySelector` (`corpus/reference/NewspeakGrammar.ns:126`,
        // `binaryMsg`). A leading `<` is still a pattern literal and a leading
        // `|` / `||` is still a temp section (both handled before we get here),
        // so the ambiguity only bites where a left operand already exists.
        // `|` / `||` double as slot/temp-section delimiters, so accept them as a
        // binary operator only when a primary can actually follow. A
        // section-closing `|` / `||` is followed by `RParen` / a decl keyword /
        // `Eoi` / another `|` — none of which start a primary — so a dot-less
        // final slot initializer no longer eats its closing delimiter and
        // cascades into the following declaration. `= < >` and plain
        // `BinarySelector` are unconditional (their non-operator uses cannot
        // reach here with a left operand present).
        while matches!(
            self.peek(),
            T::BinarySelector | T::Equals | T::LessThan | T::GreaterThan
        ) || (matches!(self.peek(), T::Bar | T::DoubleBar) && starts_primary(self.peek2()))
        {
            let op = self.bump();
            let arg = self.parse_unary_message();
            let span = expr_span(&recv).merge(expr_span(&arg));
            recv = Expr::Send(Box::new(Send {
                span,
                receiver: recv,
                message: MessageSend {
                    selector: Selector::Binary(self.text(op).to_string()),
                    args: vec![arg],
                    span,
                },
            }));
        }
        recv
    }

    pub(super) fn parse_unary_message(&mut self) -> Expr {
        let mut recv = self.parse_primary();
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
                && !self.is_reserved_ident(self.cur())
                && self.peek2() != T::Assign)
            {
                break;
            }
            let sel = self.bump();
            let span = expr_span(&recv).merge(sel.span);
            recv = Expr::Send(Box::new(Send {
                span,
                receiver: recv,
                message: MessageSend {
                    selector: Selector::Unary(self.text(sel).to_string()),
                    args: Vec::new(),
                    span,
                },
            }));
            // `foo bar[E]` — type arguments on a unary send result.
            self.skip_type_arguments();
        }
        recv
    }

    // A single `; msg` element: unary | binary | keyword, no receiver.
    fn parse_one_message_send(&mut self) -> MessageSend {
        let start = self.cur().span;
        match self.peek() {
            T::Keyword | T::SetterKeyword => {
                let mut selector = String::new();
                let mut args = Vec::new();
                let mut end = start;
                while self.at(T::Keyword) || self.at(T::SetterKeyword) {
                    let kw = self.bump();
                    selector.push_str(self.text(kw).trim_end_matches(':'));
                    selector.push(':');
                    let arg = self.parse_binary_message();
                    end = expr_span(&arg);
                    args.push(arg);
                }
                MessageSend {
                    selector: Selector::Keyword(selector),
                    args,
                    span: start.merge(end),
                }
            }
            T::BinarySelector => {
                let op = self.bump();
                let arg = self.parse_unary_message();
                MessageSend {
                    selector: Selector::Binary(self.text(op).to_string()),
                    span: start.merge(expr_span(&arg)),
                    args: vec![arg],
                }
            }
            T::Class => {
                let sel = self.bump();
                MessageSend {
                    selector: Selector::Unary(self.text(sel).to_string()),
                    args: Vec::new(),
                    span: sel.span,
                }
            }
            _ => {
                let sel = self.expect(T::Identifier).unwrap_or(self.cur());
                MessageSend {
                    selector: Selector::Unary(self.text(sel).to_string()),
                    args: Vec::new(),
                    span: sel.span,
                }
            }
        }
    }

    pub(super) fn is_reserved_ident(&self, tok: crate::token::Token) -> bool {
        matches!(self.text(tok), "nil" | "true" | "false")
    }

    /// The eventual-send operator `<-:` (`NewspeakGrammar.ns:50`), which the
    /// lexer emits as `BinarySelector("<-")` followed by `Colon`.
    pub(super) fn at_eventual_send_operator(&self) -> bool {
        self.at(T::BinarySelector) && self.text(self.cur()) == "<-" && self.peek2() == T::Colon
    }
}

/// Whether a token of this kind can begin a `primary` (`NewspeakGrammar.ns:123`
/// plus the parser's `nil`/`true`/`false`, `class` pseudo-variable and `<...>`
/// pattern-literal extensions). Used to disambiguate `|` / `||` as a binary
/// operator from a slot/temp-section delimiter.
fn starts_primary(k: T) -> bool {
    matches!(
        k,
        T::Identifier
            | T::Class
            | T::Number
            | T::String
            | T::Symbol
            | T::CharConstant
            | T::LParen
            | T::LBracket
            | T::LBrace
            | T::Pound
            | T::LessThan
    )
}

pub(super) fn expr_span(e: &Expr) -> crate::span::Span {
    match e {
        Expr::Ident(i) => i.span,
        Expr::Literal(l) => literal_span(l),
        Expr::Send(s) => s.span,
        Expr::Cascade(c) => c.span,
        Expr::Assign { span, .. } => *span,
        Expr::Block(b) => b.span,
        Expr::DynamicArray { span, .. } => *span,
    }
}

pub(super) fn literal_span(l: &Literal) -> crate::span::Span {
    match l {
        Literal::Int { span, .. }
        | Literal::Float { span, .. }
        | Literal::String { span, .. }
        | Literal::Symbol { span, .. }
        | Literal::Char { span, .. }
        | Literal::Bool { span, .. }
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
