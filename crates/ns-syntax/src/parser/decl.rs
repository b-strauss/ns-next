//! Declaration-level parsing: code bodies (and, in later tasks, classes and
//! methods).

use super::Parser;
use crate::ast::*;
use crate::token::TokenKind as T;

impl<'a> Parser<'a> {
    /// Optional `| temps |` then `Dot`-separated statements, where the last
    /// statement may be `^ expr`. A trailing `Dot` is allowed. Stops at
    /// `RBracket` / `RParen` / `Eoi`.
    pub(crate) fn parse_code_body(&mut self) -> CodeBody {
        let start = self.cur().span;
        let temps = self.parse_temps();
        let mut stmts = Vec::new();
        loop {
            match self.peek() {
                T::RBracket | T::RParen | T::Eoi => break,
                T::Caret => {
                    self.bump(); // `^`
                    let e = self.parse_expression();
                    stmts.push(Stmt::Return(e));
                    self.eat(T::Dot); // optional trailing dot
                    break; // nothing after a return
                }
                T::Dot => {
                    self.bump(); // stray dot
                }
                _ => {
                    let e = self.parse_expression();
                    stmts.push(Stmt::Expr(e));
                    if !self.eat(T::Dot) {
                        break;
                    }
                }
            }
        }
        let end = self.cur().span;
        CodeBody {
            temps,
            stmts,
            span: start.merge(end),
        }
    }

    fn parse_temps(&mut self) -> Vec<Ident> {
        if self.at(T::DoubleBar) {
            self.bump(); // `||` == empty temp list
            return Vec::new();
        }
        if !self.at(T::Bar) {
            return Vec::new();
        }
        // Distinguish `| t |` (temps) from a leading binary `|` expression:
        // a temp list is `Bar Identifier* Bar`. Look ahead.
        let mut i = self.pos + 1;
        loop {
            match self.tokens.get(i).map(|t| t.kind) {
                Some(T::Identifier) => i += 1,
                Some(T::Bar) => break,
                _ => return Vec::new(), // not a temp list
            }
        }
        self.bump(); // opening Bar
        let mut temps = Vec::new();
        while self.at(T::Identifier) {
            temps.push(self.ident());
        }
        self.expect(T::Bar);
        temps
    }

    pub(crate) fn parse_access_modifier(&mut self) -> Option<AccessModifier> {
        match self.peek() {
            T::Public => {
                self.bump();
                Some(AccessModifier::Public)
            }
            T::Protected => {
                self.bump();
                Some(AccessModifier::Protected)
            }
            T::Private => {
                self.bump();
                Some(AccessModifier::Private)
            }
            _ => None,
        }
    }

    pub(crate) fn parse_message_pattern(&mut self) -> MessagePattern {
        match self.peek() {
            T::BinarySelector => {
                let sel = self.ident_from_bump();
                let param = self.ident();
                MessagePattern::Binary {
                    selector: sel,
                    param,
                }
            }
            T::Keyword | T::SetterKeyword => {
                let mut parts = Vec::new();
                while self.at(T::Keyword) || self.at(T::SetterKeyword) {
                    let kw = self.bump();
                    let key = Ident {
                        name: self.text(kw).trim_end_matches(':').to_string(),
                        span: kw.span,
                    };
                    let param = self.ident();
                    parts.push((key, param));
                }
                MessagePattern::Keyword { parts }
            }
            _ => {
                let sel = self.ident();
                MessagePattern::Unary { selector: sel }
            }
        }
    }

    pub(crate) fn parse_method_decl(&mut self) -> MethodDecl {
        let start = self.cur().span;
        let access = self.parse_access_modifier();
        let pattern = self.parse_message_pattern();
        self.expect(T::Equals);
        self.expect(T::LParen);
        let body = self.parse_code_body();
        let end = self.cur().span;
        self.expect(T::RParen);
        MethodDecl {
            access,
            pattern,
            body,
            span: start.merge(end),
        }
    }

    fn ident_from_bump(&mut self) -> Ident {
        let t = self.bump();
        Ident {
            name: self.text(t).to_string(),
            span: t.span,
        }
    }
}

#[cfg(test)]
impl<'a> Parser<'a> {
    pub(crate) fn parse_method_decl_for_test(&mut self) -> MethodDecl {
        self.parse_method_decl()
    }

    pub(crate) fn diags_is_empty(&self) -> bool {
        self.diags.is_empty()
    }

    pub(crate) fn debug_diags(&self) -> String {
        format!("{:?}", self.diags)
    }
}

#[cfg(test)]
mod method_tests {
    use super::*;
    use crate::parser::Parser;

    fn method(src: &str) -> MethodDecl {
        let tokens = crate::lex(src).unwrap();
        let mut p = Parser::new(src, tokens);
        let m = p.parse_method_decl_for_test();
        assert!(p.diags_is_empty(), "{:?}", p.debug_diags());
        m
    }

    #[test]
    fn unary_method() {
        let m = method("greeting = ( ^ 'hi' )");
        assert!(matches!(m.pattern, MessagePattern::Unary { .. }));
        assert_eq!(m.body.stmts.len(), 1);
    }

    #[test]
    fn keyword_method_with_access() {
        let m = method("public at: k put: v = ( slot at: k put: v. ^ v )");
        let MessagePattern::Keyword { parts } = &m.pattern else {
            panic!()
        };
        assert_eq!(parts.len(), 2);
        assert_eq!(m.access, Some(AccessModifier::Public));
    }

    #[test]
    fn binary_method() {
        let m = method("+ other = ( ^ self addTo: other )");
        assert!(matches!(m.pattern, MessagePattern::Binary { .. }));
    }
}
