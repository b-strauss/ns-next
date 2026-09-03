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

    // Placeholder until Task 9.
    fn parse_block(&mut self) -> Expr {
        let lb = self.bump();
        // minimal: skip to matching ]
        while !self.at(T::RBracket) && !self.at(T::Eoi) {
            self.bump();
        }
        let rb = self.bump();
        Expr::Block(Box::new(BlockExpr {
            params: Vec::new(),
            body: CodeBody {
                temps: Vec::new(),
                stmts: Vec::new(),
                span: lb.span.merge(rb.span),
            },
            span: lb.span.merge(rb.span),
        }))
    }

    fn parse_dynamic_array(&mut self) -> Expr {
        let lb = self.bump();
        while !self.at(T::RBrace) && !self.at(T::Eoi) {
            self.bump();
        }
        let rb = self.bump();
        Expr::DynamicArray {
            elements: Vec::new(),
            span: lb.span.merge(rb.span),
        }
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
