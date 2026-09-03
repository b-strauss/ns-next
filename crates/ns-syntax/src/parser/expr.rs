use super::Parser;
use crate::ast::*;
use crate::token::TokenKind as T;

impl<'a> Parser<'a> {
    pub(super) fn parse_primary(&mut self) -> Expr {
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
                let inner = &raw[1..raw.len() - 1];
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

    fn parse_array_element(&mut self) -> Literal {
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
        let mut recv = self.parse_binary_message();
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

    fn parse_binary_message(&mut self) -> Expr {
        let mut recv = self.parse_unary_message();
        while self.at(T::BinarySelector) {
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

    fn parse_unary_message(&mut self) -> Expr {
        let mut recv = self.parse_primary();
        while self.at(T::Identifier)
            && !self.is_reserved_ident(self.cur())
            && self.peek2() != T::Assign
        {
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

    fn is_reserved_ident(&self, tok: crate::token::Token) -> bool {
        matches!(self.text(tok), "nil" | "true" | "false")
    }
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
    let inner = &raw[1..raw.len() - 1];
    inner.replace("''", "'")
}

fn symbol_body(raw: &str) -> String {
    let body = &raw[1..]; // drop '#'
    if body.starts_with('\'') {
        body[1..body.len() - 1].replace("''", "'")
    } else {
        body.to_string()
    }
}
