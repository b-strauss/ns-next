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
}
