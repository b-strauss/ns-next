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

    pub(crate) fn parse_slot_decls(&mut self) -> Vec<SlotDecl> {
        let simultaneous = self.at(T::DoubleBar);
        let close = if simultaneous { T::DoubleBar } else { T::Bar };
        if !self.eat(close) {
            return Vec::new();
        }
        let mut slots = Vec::new();
        while !self.at(close) && !self.at(T::Eoi) {
            let start = self.cur().span;
            let access = self.parse_access_modifier();
            if !self.at(T::Identifier) {
                // recovery: skip to next '.' or close
                let t = self.bump();
                self.push_error(t.span, "expected a slot name");
                self.recover_to_dot(close);
                continue;
            }
            let name = self.ident();
            let (mutability, init) = match self.peek() {
                T::Equals => {
                    self.bump();
                    (SlotMutability::Immutable, Some(self.parse_expression()))
                }
                T::ColonColonEquals => {
                    self.bump();
                    (SlotMutability::MutableInit, Some(self.parse_expression()))
                }
                _ => (SlotMutability::MutableBare, None),
            };
            let end = self.cur().span;
            self.eat(T::Dot);
            slots.push(SlotDecl {
                access,
                name,
                mutability,
                init,
                span: start.merge(end),
            });
        }
        self.expect(close);
        slots
    }

    fn recover_to_dot(&mut self, close: T) {
        while !self.at(T::Dot) && !self.at(close) && !self.at(T::Eoi) {
            self.bump();
        }
        self.eat(T::Dot);
    }

    fn ident_from_bump(&mut self) -> Ident {
        let t = self.bump();
        Ident {
            name: self.text(t).to_string(),
            span: t.span,
        }
    }
}

impl<'a> Parser<'a> {
    /// Top level: `languageId category classDecl Eoi`.
    ///
    /// `languageId` is the first `Identifier` token (`Newspeak3`); `category`
    /// is the first `String` literal. Missing pieces and trailing tokens are
    /// reported but parsing continues (recovery).
    pub(crate) fn parse_module(&mut self) -> Module {
        let start = self.cur().span;
        let language_id = if self.at(T::Identifier) {
            let t = self.bump();
            self.text(t).to_string()
        } else {
            self.push_error(self.cur().span, "expected a language id (e.g. `Newspeak3`)");
            String::new()
        };
        let category = if self.at(T::String) {
            let t = self.bump();
            string_lexeme_body(self.text(t))
        } else {
            self.push_error(self.cur().span, "expected a category string");
            String::new()
        };
        let root_class = self.parse_class_decl();
        let end = self.cur().span;
        if !self.at(T::Eoi) {
            self.push_error(
                self.cur().span,
                "unexpected tokens after the top-level class",
            );
        }
        Module {
            language_id,
            category,
            root_class,
            span: start.merge(end),
        }
    }

    /// `accessModifier? 'class' Ident messagePattern? '=' superclassClause? '('
    /// classComment? classBody ')' (':' '(' classComment? classBody ')')?`
    pub(crate) fn parse_class_decl(&mut self) -> ClassDecl {
        let start = self.cur().span;
        let access = self.parse_access_modifier();
        self.expect(T::Class);
        let name = self.ident();
        let factory = if self.at(T::Equals) {
            None
        } else {
            Some(self.parse_message_pattern())
        };
        self.expect(T::Equals);
        let superclass = if self.at(T::LParen) {
            None
        } else {
            Some(self.parse_superclass_clause())
        };
        self.expect(T::LParen);
        let class_comment = self.take_class_comment();
        let instance_side = self.parse_class_body(true);
        let inst_end = self.cur().span;
        self.expect(T::RParen);
        let class_side = if self.eat(T::Colon) {
            self.expect(T::LParen);
            // class-side comment is allowed too, but not retained
            let _ = self.take_class_comment();
            let body = self.parse_class_body(false);
            self.expect(T::RParen);
            Some(body)
        } else {
            None
        };
        let end = class_side.as_ref().map(|b| b.span).unwrap_or(inst_end);
        ClassDecl {
            access,
            name,
            factory,
            superclass,
            class_comment,
            instance_side,
            class_side,
            span: start.merge(end),
        }
    }

    /// A class body may open with a comment: either a `'...'` string literal
    /// that stands alone, or a `(* ... *)` block comment. The lexer drops block
    /// comments as trivia, so that form is recovered from the source gap
    /// between the opening `(` and the next token.
    fn take_class_comment(&mut self) -> Option<String> {
        if self.at(T::String) {
            let t = self.bump();
            return Some(string_lexeme_body(self.text(t)));
        }
        let prev_end = self.tokens.get(self.pos.checked_sub(1)?)?.span.end as usize;
        let cur_start = self.cur().span.start as usize;
        let gap = self.src.get(prev_end..cur_start)?.trim();
        let inner = gap.strip_prefix("(*")?.strip_suffix("*)")?;
        Some(inner.to_string())
    }

    /// `superclassPrefix? Ident message?` — the receiver identifier followed by
    /// zero or more unary/binary/keyword sends (`Object new`,
    /// `platform collections List`, `superclass subclassMixin: m`).
    pub(crate) fn parse_superclass_clause(&mut self) -> SuperclassClause {
        let start = self.cur().span;
        let prefix = match self.peek() {
            T::Identifier => match self.text(self.cur()) {
                "self" => {
                    self.bump();
                    Some(SuperclassPrefix::SelfPrefix)
                }
                "super" => {
                    self.bump();
                    Some(SuperclassPrefix::SuperPrefix)
                }
                "outer" => {
                    self.bump();
                    Some(SuperclassPrefix::Outer(self.ident()))
                }
                _ => None,
            },
            _ => None,
        };
        let receiver = self.ident();
        let mut messages = Vec::new();
        self.parse_trailing_sends(&mut messages);
        let end = messages.last().map(|m| m.span).unwrap_or(receiver.span);
        SuperclassClause {
            prefix,
            receiver,
            messages,
            span: start.merge(end),
        }
    }

    /// unary*, then binary*, then an optional keyword send — the grammar's
    /// `message`, seeded by an already-parsed receiver.
    fn parse_trailing_sends(&mut self, out: &mut Vec<MessageSend>) {
        while self.at(T::Identifier)
            && !self.is_reserved_ident(self.cur())
            && self.peek2() != T::Assign
            && self.peek() != T::Equals
        {
            let sel = self.bump();
            out.push(MessageSend {
                selector: Selector::Unary(self.text(sel).to_string()),
                args: Vec::new(),
                span: sel.span,
            });
        }
        while self.at(T::BinarySelector) {
            let op = self.bump();
            let arg = self.parse_unary_message();
            out.push(MessageSend {
                selector: Selector::Binary(self.text(op).to_string()),
                span: op.span.merge(super::expr::expr_span(&arg)),
                args: vec![arg],
            });
        }
        if self.at(T::Keyword) {
            let mut selector = String::new();
            let mut args = Vec::new();
            let start = self.cur().span;
            let mut end = start;
            while self.at(T::Keyword) {
                let kw = self.bump();
                selector.push_str(self.text(kw).trim_end_matches(':'));
                selector.push(':');
                let arg = self.parse_binary_message();
                end = super::expr::expr_span(&arg);
                args.push(arg);
            }
            out.push(MessageSend {
                selector: Selector::Keyword(selector),
                args,
                span: start.merge(end),
            });
        }
    }

    /// The interleaving inside `( ... )`: an optional slot section, then a
    /// sequence of nested classes / method decls / (instance side, before the
    /// first decl) `.`-terminated init expressions.
    pub(crate) fn parse_class_body(&mut self, instance_side: bool) -> ClassBody {
        let start = self.cur().span;
        let slots = if self.at(T::Bar) || self.at(T::DoubleBar) {
            self.parse_slot_decls()
        } else {
            Vec::new()
        };
        let mut init_exprs = Vec::new();
        let mut methods = Vec::new();
        let mut nested_classes = Vec::new();
        let mut seen_decl = false;
        loop {
            match self.peek() {
                T::RParen | T::Eoi => break,
                T::Class | T::Public | T::Protected | T::Private => {
                    if self.peek() == T::Class || self.class_after_modifier() {
                        nested_classes.push(self.parse_class_decl());
                    } else {
                        methods.push(self.parse_method_decl());
                    }
                    seen_decl = true;
                }
                T::Dot => {
                    self.bump();
                }
                _ => {
                    if !seen_decl && self.looks_like_method() {
                        methods.push(self.parse_method_decl());
                        seen_decl = true;
                    } else if !seen_decl && instance_side {
                        let e = self.parse_expression();
                        init_exprs.push(Stmt::Expr(e));
                        self.eat(T::Dot);
                    } else if self.looks_like_method() {
                        methods.push(self.parse_method_decl());
                        seen_decl = true;
                    } else {
                        let t = self.bump();
                        self.push_error(
                            t.span,
                            "expected a method, nested class, or (before them) an init expression",
                        );
                    }
                }
            }
        }
        let end = self.cur().span;
        ClassBody {
            slots,
            init_exprs,
            methods,
            nested_classes,
            span: start.merge(end),
        }
    }

    fn class_after_modifier(&self) -> bool {
        self.peek2() == T::Class
    }

    /// Speculative lookahead: does the cursor sit at `accessModifier?
    /// messagePattern '=' '('` (a method), rather than an init expression?
    fn looks_like_method(&mut self) -> bool {
        let cp = self.checkpoint();
        self.parse_access_modifier();
        let _ = self.try_parse_message_pattern();
        let is_method =
            self.at(T::Equals) && self.tokens.get(self.pos + 1).map(|t| t.kind) == Some(T::LParen);
        self.restore(cp);
        is_method
    }

    fn try_parse_message_pattern(&mut self) -> Option<()> {
        match self.peek() {
            T::BinarySelector => {
                self.bump();
                if self.at(T::Identifier) {
                    self.bump();
                }
                Some(())
            }
            T::Keyword | T::SetterKeyword => {
                while self.at(T::Keyword) || self.at(T::SetterKeyword) {
                    self.bump();
                    if self.at(T::Identifier) {
                        self.bump();
                    } else {
                        return None;
                    }
                }
                Some(())
            }
            T::Identifier => {
                self.bump();
                Some(())
            }
            _ => None,
        }
    }
}

/// Contents of a `'...'` string lexeme with `''` unescaped. Safe on a
/// truncated lexeme (returns "" rather than panicking).
fn string_lexeme_body(raw: &str) -> String {
    raw.get(1..raw.len().saturating_sub(1))
        .unwrap_or("")
        .replace("''", "'")
}

#[cfg(test)]
impl<'a> Parser<'a> {
    pub(crate) fn parse_method_decl_for_test(&mut self) -> MethodDecl {
        self.parse_method_decl()
    }

    pub(crate) fn parse_slot_decls_for_test(&mut self) -> Vec<SlotDecl> {
        self.parse_slot_decls()
    }

    pub(crate) fn diags_is_empty(&self) -> bool {
        self.diags.is_empty()
    }

    pub(crate) fn debug_diags(&self) -> String {
        format!("{:?}", self.diags)
    }
}

#[cfg(test)]
mod slot_tests {
    use super::*;
    use crate::parser::Parser;

    fn slots(src: &str) -> Vec<SlotDecl> {
        let tokens = crate::lex(src).unwrap();
        let mut p = Parser::new(src, tokens);
        let s = p.parse_slot_decls_for_test();
        assert!(p.diags_is_empty(), "{:?}", p.debug_diags());
        s
    }

    #[test]
    fn immutable_and_mutable_slots() {
        let s = slots("| private List = platform collections List. count ::= 0. name. |");
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].access, Some(AccessModifier::Private));
        assert_eq!(s[0].mutability, SlotMutability::Immutable);
        assert_eq!(s[1].mutability, SlotMutability::MutableInit);
        assert_eq!(s[2].mutability, SlotMutability::MutableBare);
    }

    #[test]
    fn empty_section() {
        assert!(slots("| |").is_empty());
    }

    #[test]
    fn simultaneous_section() {
        let s = slots("|| a = 1. b = 2. ||");
        assert_eq!(s.len(), 2);
    }
}

#[cfg(test)]
mod class_tests {
    use super::*;

    fn module(src: &str) -> Module {
        match crate::parse(src) {
            Ok(m) => m,
            Err(d) => panic!("parse errors: {d:#?}"),
        }
    }

    #[test]
    fn minimal_module() {
        let m = module("Newspeak3\n'App'\nclass Empty usingPlatform: p = ()\n");
        assert_eq!(m.language_id, "Newspeak3");
        assert_eq!(m.category, "App");
        assert_eq!(m.root_class.name.name, "Empty");
        let MessagePattern::Keyword { parts } = &m.root_class.factory.as_ref().unwrap() else {
            panic!()
        };
        assert_eq!(parts[0].0.name, "usingPlatform");
    }

    #[test]
    fn superclass_clause_with_send() {
        let m = module("Newspeak3\n'X'\nclass C = Object new ()\n");
        let sc = m.root_class.superclass.as_ref().unwrap();
        assert_eq!(sc.receiver.name, "Object");
        assert_eq!(sc.messages.len(), 1);
    }

    #[test]
    fn slots_methods_and_nested_class() {
        let src = r#"Newspeak3
'X'
class Outer usingPlatform: p = (
    (* a comment *)
    | x ::= 0. |
    x := 1.
    public bump = ( x := x + 1 )
    class Inner = ( public ping = ( ^ 42 ) )
)"#;
        let m = module(src);
        let body = &m.root_class.instance_side;
        assert_eq!(body.slots.len(), 1);
        assert_eq!(body.init_exprs.len(), 1);
        assert_eq!(body.methods.len(), 1);
        assert_eq!(body.nested_classes.len(), 1);
        assert_eq!(m.root_class.class_comment.as_deref(), Some(" a comment "));
    }

    #[test]
    fn class_side_block() {
        let src = "Newspeak3\n'X'\nclass C = ( ) : ( public default = ( ^ C new ) )\n";
        let m = module(src);
        assert_eq!(m.root_class.class_side.as_ref().unwrap().methods.len(), 1);
    }

    #[test]
    fn trailing_garbage_is_an_error() {
        assert!(crate::parse("Newspeak3\n'X'\nclass C = ()\nextra").is_err());
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
