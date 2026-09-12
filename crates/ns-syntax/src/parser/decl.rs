//! Declaration-level parsing: classes, methods, slots and code bodies.

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

    /// `codeBody` temporaries. Per the grammar
    /// (`corpus/reference/NewspeakGrammar.ns:148-150`), `temporaries =
    /// slotDecls`, i.e. a temp list has the full slot-declaration form,
    /// including initialized locals `| x = expr. |`. Milestone 1's
    /// `CodeBody::temps` keeps only the names: a local initializer is parsed
    /// (so the rest of the body stays in sync) but then dropped, the same way
    /// `lazy` is dropped elsewhere. A code body never legitimately starts with
    /// a binary `|` send (there is no left operand), so a leading `Bar` /
    /// `DoubleBar` is unambiguously a temp section.
    fn parse_temps(&mut self) -> Vec<Ident> {
        // The lexer emits `||` as one `DoubleBar` token. In code-body position
        // that lone token is the degenerate empty simultaneous temp list —
        // short-circuit it before the `parse_slot_decls` handoff, which would
        // otherwise consume `||` and then scan the rest of the body hunting a
        // closing `||` that never comes.
        if self.at(T::DoubleBar) {
            self.bump();
            return Vec::new();
        }
        if !self.at(T::Bar) {
            return Vec::new();
        }
        self.parse_slot_decls()
            .into_iter()
            .map(|s| s.name)
            .collect()
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

    /// `=`, `<` and `>` lex as the dedicated punctuation tokens `Equals` /
    /// `LessThan` / `GreaterThan` (they carry declaration syntax), but the
    /// grammar's `binarySelector` (`corpus/reference/NewspeakGrammar.ns:126`)
    /// includes them, so `public = other = (...)` / `< other = (...)` /
    /// `> other = (...)` are binary-operator method declarations. In
    /// message-pattern position such a token followed by an identifier is a
    /// binary selector, not the pattern's `=` terminator.
    fn at_binary_selector_pattern(&self) -> bool {
        matches!(
            self.peek(),
            T::BinarySelector | T::Equals | T::LessThan | T::GreaterThan | T::Bar
        ) && self.peek2() == T::Identifier
    }

    pub(crate) fn parse_message_pattern(&mut self) -> MessagePattern {
        // Declarations may carry `<Type>` annotations after each parameter name,
        // and a `^<Type>` return annotation before the `=`; Milestone 1 discards
        // them via `skip_type_annotation` / `skip_return_type_annotation`.
        let pattern = if self.at_binary_selector_pattern() {
            let sel = self.ident_from_bump();
            let param = self.ident();
            self.skip_type_annotation();
            MessagePattern::Binary {
                selector: sel,
                param,
            }
        } else {
            match self.peek() {
                T::Keyword | T::SetterKeyword => {
                    let mut parts = Vec::new();
                    while self.at(T::Keyword) || self.at(T::SetterKeyword) {
                        let kw = self.bump();
                        let key = Ident {
                            name: self.text(kw).trim_end_matches(':').to_string(),
                            span: kw.span,
                        };
                        let param = self.ident();
                        self.skip_type_annotation();
                        parts.push((key, param));
                    }
                    MessagePattern::Keyword { parts }
                }
                _ => {
                    let sel = self.ident();
                    // `selectorTypeParameters` (`NewspeakGrammar.ns:446`):
                    // `public List[E] = (...)` — type parameters on the
                    // selector itself. Dropped in Milestone 1.
                    self.skip_type_arguments();
                    self.skip_type_annotation();
                    MessagePattern::Unary { selector: sel }
                }
            }
        };
        self.skip_return_type_annotation();
        self.skip_where_clause();
        pattern
    }

    /// A method/slot return type `^ <Type>` (grammar's `returnType`, e.g.
    /// `storageKey ^<String> = (...)`, `seed: v <Value> ^<FarReference[Value]>
    /// = (...)`). Milestone 1 drops it. A bare `^` with no following `<` is left
    /// in place (it is then a syntax error the caller reports).
    fn skip_return_type_annotation(&mut self) {
        if self.at(T::Caret) && self.peek2() == T::LessThan {
            self.bump(); // `^`
            self.skip_type_annotation();
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
            // A slot / temporary may be named `class` (`| class |` locals in
            // the mirror libraries); `class` lexes as `T::Class`.
            if !self.at(T::Identifier) && !self.at(T::Class) {
                // recovery: skip to next '.' or close
                let t = self.bump();
                self.push_error(t.span, "expected a slot name");
                self.recover_to_dot(close);
                continue;
            }
            let name = self.ident();
            // `slotName <Type>` — the annotation is dropped in Milestone 1.
            self.skip_type_annotation();
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
        // DEVIATION (grammar line 202: `classCategory = string opt`): the
        // grammar makes the category string optional; we require it and emit a
        // diagnostic when it is absent. Every corpus module carries one;
        // revisit in Milestone 2.
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

    /// The real Newspeak3 class grammar
    /// (`corpus/reference/NewspeakGrammar.ns:189,200,208,212`):
    ///
    /// ```text
    /// classDeclaration = classHeader , sideDecl , classSideDecl?
    /// classHeader   = accessModifier? 'class' classId messagePattern? '='
    ///                 superclassClause? '(' classComment? slotDecls? initExprs ')'
    /// sideDecl      = '(' ( classDecl | lazySlotDecl | methodDecl )* ')'
    /// classSideDecl = ':' '(' methodDecl* ')'
    /// ```
    ///
    /// So a class is `class Id pattern? = superclause? ( G1 ) ( G2 ) ( ':' '(' G3 ')' )?`
    /// with three distinct paren groups:
    /// - **G1** (`classHeader` body): class comment, ONE slot section, then
    ///   init expressions. No methods, no nested classes.
    /// - **G2** (`sideDecl` body): nested classes, methods, and lazy-slot decls.
    /// - **G3** (`classSideDecl` body): methods only.
    ///
    /// The AST keeps a single `instance_side: ClassBody`: G1's slots (plus any
    /// G2 lazy-slot decls) land in `slots`, G1's init exprs in `init_exprs`,
    /// and G2's methods / nested classes in `methods` / `nested_classes`.
    /// `class_side` is G3.
    pub(crate) fn parse_class_decl(&mut self) -> ClassDecl {
        let start = self.cur().span;
        let access = self.parse_access_modifier();
        self.expect(T::Class);
        let name = self.ident();
        // `class List[E] ...` — generic type-parameter declaration
        // (`NewspeakGrammar.ns:465`); dropped in Milestone 1.
        self.skip_type_arguments();
        let factory = if self.at(T::Equals) {
            None
        } else {
            Some(self.parse_message_pattern())
        };
        self.expect(T::Equals);
        // DEVIATION (grammar lines 49, 195, 214-221): mixin application
        // `class C = Super <: Mixin (...) (...)` (the `inverseMixinOperator`
        // `<:`, `mixinApplication` / `mixinAppSuffix`) is unimplemented for
        // Milestone 1 — no corpus file uses `<:` in a superclass clause.
        // `parse_superclass_clause` parses the receiver and its trailing
        // message sends only; a following `<:` is left for the caller to flag.
        let superclass = if self.at(T::LParen) {
            None
        } else {
            Some(self.parse_superclass_clause())
        };

        // --- G1: classHeader body `( classComment? slotDecls? initExprs )` ---
        self.expect(T::LParen);
        let class_comment = self.take_class_comment();
        let (header_slots, init_exprs, g1_span) = self.parse_header_body();
        let g1_end = self.cur().span;
        self.expect(T::RParen);

        // --- G2: sideDecl body `( ( classDecl | lazySlotDecl | methodDecl )* )` ---
        // Corpus always has G2; being lenient when it is absent keeps a
        // degenerate `class C = ()` parseable.
        let (lazy_slots, methods, nested_classes, g2_end) = if self.at(T::LParen) {
            self.bump(); // `(`
            let (lazy, methods, nested) = self.parse_side_body();
            let end = self.cur().span;
            self.expect(T::RParen);
            (lazy, methods, nested, end)
        } else {
            (Vec::new(), Vec::new(), Vec::new(), g1_end)
        };

        let mut slots = header_slots;
        slots.extend(lazy_slots);
        let instance_side = ClassBody {
            slots,
            init_exprs,
            methods,
            nested_classes,
            span: g1_span.merge(g2_end),
        };

        // --- G3: classSideDecl body `: ( methodDecl* )` ---
        let class_side = if self.eat(T::Colon) {
            self.expect(T::LParen);
            // A class-side comment is allowed but not retained.
            let _ = self.take_class_comment();
            let body = self.parse_class_side_body();
            self.expect(T::RParen);
            Some(body)
        } else {
            None
        };

        let end = class_side.as_ref().map(|b| b.span).unwrap_or(g2_end);
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

    /// A class body may open with a class comment. Per the vendored grammar
    /// (`corpus/reference/NewspeakGrammar.ns:196`,
    /// `classComment = whitespace opt , comment`, where `comment` is a
    /// `(* ... *)` block form — `beginNSComment`/`endNSComment`, grammar
    /// lines 152-153) it is *exclusively* a block comment, which the lexer
    /// drops as trivia; that form is recovered here from the source gap
    /// between the opening `(` and the next token.
    ///
    /// A leading `'...'` string in the header body is NOT a class comment: per
    /// grammar line 165 it is the first `initExprs` expression (e.g. its
    /// receiver in `'hello' printLine.`). Consuming it here silently dropped a
    /// valid init expression, so that branch was removed.
    fn take_class_comment(&mut self) -> Option<String> {
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
        // `Collection[E] new` — type arguments on the superclass receiver.
        self.skip_type_arguments();
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
        while (self.at(T::Identifier) || self.at(T::Class))
            && !self.is_reserved_ident(self.cur())
            && self.peek2() != T::Assign
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

    /// **G1** — `classHeader` body (`NewspeakGrammar.ns:208`):
    /// `classComment? , slotDecls? , initExprs`. The class comment is consumed
    /// by the caller via [`Parser::take_class_comment`] right after the `(`.
    /// Here: an optional single slot section (`| .. |` or `|| .. ||`), then
    /// zero or more `Dot`-separated/terminated init expressions, until `)` or
    /// end of input. Methods, nested classes and a second slot section are NOT
    /// accepted here — they belong to G2.
    fn parse_header_body(&mut self) -> (Vec<SlotDecl>, Vec<Stmt>, crate::span::Span) {
        let start = self.cur().span;
        let slots = if self.at(T::Bar) || self.at(T::DoubleBar) {
            self.parse_slot_decls()
        } else {
            Vec::new()
        };
        let mut init_exprs = Vec::new();
        loop {
            match self.peek() {
                T::RParen | T::Eoi => break,
                T::Dot => {
                    self.bump();
                }
                _ => {
                    let before = self.pos;
                    let e = self.parse_expression();
                    self.eat(T::Dot);
                    if self.pos == before {
                        // `parse_expression` can return a recovery node without
                        // consuming a token (a stray `^ ; + = ]` etc). Force
                        // progress and drop the bogus node.
                        let t = self.bump();
                        self.push_error(t.span, "expected an init expression or `)`");
                    } else {
                        init_exprs.push(Stmt::Expr(e));
                    }
                }
            }
        }
        let end = self.cur().span;
        (slots, init_exprs, start.merge(end))
    }

    /// **G2** — `sideDecl` body (`NewspeakGrammar.ns:189`):
    /// `( classDecl | lazySlotDecl | methodDecl )*`. Returns the lazy-slot
    /// decls (merged into `ClassBody::slots` by the caller), the methods, and
    /// the nested classes. A `| .. |` slot section, a bare init expression or a
    /// class comment here is malformed (those belong to G1).
    fn parse_side_body(&mut self) -> (Vec<SlotDecl>, Vec<MethodDecl>, Vec<ClassDecl>) {
        let mut lazy_slots = Vec::new();
        let mut methods = Vec::new();
        let mut nested_classes = Vec::new();
        loop {
            match self.peek() {
                T::RParen | T::Eoi => break,
                _ => {}
            }
            let before = self.pos;
            match self.peek() {
                T::Dot => {
                    self.bump();
                }
                // A `| param ... = (...)` binary-operator method named `|` is
                // valid here; only a genuine `|...|` slot section is not.
                T::Bar | T::DoubleBar if !self.looks_like_method() => {
                    let t = self.bump();
                    self.push_error(
                        t.span,
                        "a `|...|` slot section is not allowed in the class body \
                         (grammar line 189); it belongs in the class header",
                    );
                }
                _ if self.at_nested_class() => {
                    // Recursion-depth guard for nested classes
                    // (`class C = () ( class C = () ( … `). Without it deeply
                    // nested classes recurse until an uncatchable stack
                    // overflow. On exceeding the limit: one diagnostic, skip to
                    // the nearest structural token, and stop the nested-class
                    // loop for this body.
                    if !self.enter() {
                        self.leave();
                        let t = self.cur();
                        self.push_error(t.span, "class nesting too deep");
                        self.skip_to_structural();
                        break;
                    }
                    nested_classes.push(self.parse_class_decl());
                    self.leave();
                }
                _ if self.at_lazy_slot() => {
                    lazy_slots.push(self.parse_lazy_slot_decl());
                }
                _ if self.looks_like_method() => {
                    methods.push(self.parse_method_decl());
                }
                _ => {
                    let t = self.bump();
                    self.push_error(
                        t.span,
                        "expected a method, nested class, or lazy slot declaration",
                    );
                }
            }
            if self.pos == before {
                // Forward-progress guard: nothing above consumed a token.
                let t = self.bump();
                self.push_error(t.span, "unexpected token in class body");
            }
        }
        (lazy_slots, methods, nested_classes)
    }

    /// **G3** — `classSideDecl` body (`NewspeakGrammar.ns:200`):
    /// `methodDecl*`. Methods only; slots / init exprs / nested classes stay
    /// empty.
    fn parse_class_side_body(&mut self) -> ClassBody {
        let start = self.cur().span;
        let mut methods = Vec::new();
        loop {
            match self.peek() {
                T::RParen | T::Eoi => break,
                _ => {}
            }
            let before = self.pos;
            match self.peek() {
                T::Dot => {
                    self.bump();
                }
                _ if self.looks_like_method() => {
                    methods.push(self.parse_method_decl());
                }
                _ => {
                    let t = self.bump();
                    self.push_error(t.span, "expected a method declaration");
                }
            }
            if self.pos == before {
                let t = self.bump();
                self.push_error(t.span, "unexpected token on the class side");
            }
        }
        let end = self.cur().span;
        ClassBody {
            slots: Vec::new(),
            init_exprs: Vec::new(),
            methods,
            nested_classes: Vec::new(),
            span: start.merge(end),
        }
    }

    /// A nested class is `accessModifier? 'class' classId ...` — the token
    /// after `class` must be the class-name identifier. `public class = (...)`
    /// (a method literally named `class`, common in the mirror libraries) has
    /// `=` there, not an identifier, so it is NOT a nested class.
    fn at_nested_class(&self) -> bool {
        if self.peek() == T::Class {
            return self.peek2() == T::Identifier;
        }
        if matches!(self.peek(), T::Public | T::Protected | T::Private) && self.peek2() == T::Class
        {
            return self.tokens.get(self.pos + 2).map(|t| t.kind) == Some(T::Identifier);
        }
        false
    }

    /// `lazy` lexes as an `Identifier` with text `"lazy"`. A lazy slot decl is
    /// either `lazy accessModifier? slotName ...` (lazy-first) or
    /// `accessModifier lazy slotName ...` (modifier-first) —
    /// `NewspeakGrammar.ns:183-186`.
    fn at_lazy_slot(&self) -> bool {
        if self.at(T::Identifier) && self.text(self.cur()) == "lazy" {
            // Distinguish from a method literally named `lazy` (`lazy = (...)`).
            return matches!(
                self.peek2(),
                T::Public | T::Protected | T::Private | T::Identifier
            );
        }
        if matches!(self.peek(), T::Public | T::Protected | T::Private) {
            if let Some(t1) = self.tokens.get(self.pos + 1) {
                return t1.kind == T::Identifier && self.text(*t1) == "lazy";
            }
        }
        false
    }

    /// Parse a lazy slot declaration. Milestone 1's AST has no "lazy" flag, so
    /// the laziness is dropped and the slot is recorded as an ordinary
    /// [`SlotDecl`] (mutability from `=` vs `::=`). Both grammar orderings are
    /// accepted.
    fn parse_lazy_slot_decl(&mut self) -> SlotDecl {
        let start = self.cur().span;
        let access = if self.at(T::Identifier) && self.text(self.cur()) == "lazy" {
            self.bump(); // `lazy`
            self.parse_access_modifier()
        } else {
            let a = self.parse_access_modifier();
            if self.at(T::Identifier) && self.text(self.cur()) == "lazy" {
                self.bump(); // `lazy`
            }
            a
        };
        let name = self.ident();
        self.skip_type_annotation();
        let mutability = if self.eat(T::ColonColonEquals) {
            SlotMutability::MutableInit
        } else {
            self.expect(T::Equals);
            SlotMutability::Immutable
        };
        let init = Some(self.parse_expression());
        let end = self.cur().span;
        self.eat(T::Dot);
        SlotDecl {
            access,
            name,
            mutability,
            init,
            span: start.merge(end),
        }
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
        // Mirror `parse_message_pattern`'s `<Type>` / `^<Type>` skipping and its
        // `= < >`-as-binary-selector handling so the speculative
        // `looks_like_method` lookahead lands on `=` `(`, not `<` / `^`.
        let matched = if self.at_binary_selector_pattern() {
            self.bump(); // selector
            self.bump(); // param ident
            self.skip_type_annotation();
            Some(())
        } else {
            match self.peek() {
                T::Keyword | T::SetterKeyword => {
                    while self.at(T::Keyword) || self.at(T::SetterKeyword) {
                        self.bump();
                        if self.at(T::Identifier) {
                            self.bump();
                        } else {
                            return None;
                        }
                        self.skip_type_annotation();
                    }
                    Some(())
                }
                T::Identifier | T::Class => {
                    self.bump();
                    self.skip_type_arguments();
                    self.skip_type_annotation();
                    Some(())
                }
                _ => None,
            }
        };
        matched?;
        self.skip_return_type_annotation();
        self.skip_where_clause();
        Some(())
    }
}

/// Contents of a `'...'` string lexeme with `''` unescaped. Safe on a
/// truncated lexeme (returns the input unchanged rather than panicking) via
/// the shared [`super::lexeme_body`] helper.
fn string_lexeme_body(raw: &str) -> String {
    super::lexeme_body(raw).replace("''", "'")
}

#[cfg(test)]
impl<'a> Parser<'a> {
    pub(crate) fn parse_method_decl_for_test(&mut self) -> MethodDecl {
        self.parse_method_decl()
    }

    pub(crate) fn parse_slot_decls_for_test(&mut self) -> Vec<SlotDecl> {
        self.parse_slot_decls()
    }

    pub(crate) fn parse_module_for_test(&mut self) -> Module {
        self.parse_module()
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

    // --- Slot type annotations, and `class` as a name ---

    #[test]
    fn slot_type_annotation() {
        let s = slots("| public foo <Bar> = 1. x <Baz[Y]> ::= 2. n <Q>. |");
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].name.name, "foo");
        assert_eq!(s[0].mutability, SlotMutability::Immutable);
        assert_eq!(s[1].mutability, SlotMutability::MutableInit);
        assert_eq!(s[2].mutability, SlotMutability::MutableBare);
    }

    #[test]
    fn slot_named_class() {
        // `| class |` locals appear in the mirror libraries.
        let s = slots("| class |");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name.name, "class");
    }
}

#[cfg(test)]
mod class_tests {
    use super::*;
    use indoc::indoc;

    fn module(src: &str) -> Module {
        match crate::parse(src) {
            Ok(m) => m,
            Err(d) => panic!("parse errors: {d:#?}"),
        }
    }

    // Parse a module keeping the (possibly partial) AST *and* the diagnostics,
    // for exercising error recovery.
    fn module_with_diags(src: &str) -> (Module, bool) {
        let tokens = crate::lex(src).unwrap();
        let mut p = super::Parser::new(src, tokens);
        let m = p.parse_module_for_test();
        let clean = p.diags_is_empty();
        (m, clean)
    }

    // A dot-less final slot initializer (invalid — grammar :165 makes the
    // trailing `.` mandatory) must not have its section-closing `|` eaten as
    // a binary `|` operator, which cascades into and swallows the following
    // G2 declaration.
    #[test]
    fn dotless_final_slot_init_does_not_swallow_following_decl() {
        let src = indoc! {"
            Newspeak3
            'X'
            class Outer = (
             | a = X new |
            ) (
             class Inner = () () : ()
            ) : ()
        "};
        let (m, _clean) = module_with_diags(src);
        let body = &m.root_class.instance_side;
        assert_eq!(body.slots.len(), 1, "the slot survives");
        assert_eq!(body.slots[0].name.name, "a");
        assert_eq!(
            body.nested_classes.len(),
            1,
            "the following nested class is not swallowed"
        );
        assert_eq!(body.nested_classes[0].name.name, "Inner");
    }

    // The boolean-or case still works: `|` with a primary after it is a binary
    // send even in a slot initializer.
    #[test]
    fn bar_operator_in_slot_initializer() {
        let m = module(indoc! {"
            Newspeak3
            'X'
            class C = (
             | flag = a | b. |
            ) ()
        "});
        let init = m.root_class.instance_side.slots[0].init.as_ref().unwrap();
        let Expr::Send(s) = init else {
            panic!("{init:?}")
        };
        assert_eq!(s.message.selector, Selector::Binary("|".into()));
        let Expr::Ident(arg) = &s.message.args[0] else {
            panic!()
        };
        assert_eq!(arg.name, "b");
    }

    #[test]
    fn minimal_module() {
        // Real grammar: G1 `()` then G2 `()` (both empty).
        let m = module(indoc! {"
            Newspeak3
            'App'
            class Empty usingPlatform: p = () ()
        "});
        assert_eq!(m.language_id, "Newspeak3");
        assert_eq!(m.category, "App");
        assert_eq!(m.root_class.name.name, "Empty");
        let MessagePattern::Keyword { parts } = &m.root_class.factory.as_ref().unwrap() else {
            panic!()
        };
        assert_eq!(parts[0].0.name, "usingPlatform");
        assert!(m.root_class.instance_side.slots.is_empty());
        assert!(m.root_class.instance_side.methods.is_empty());
    }

    #[test]
    fn superclass_clause_with_send() {
        let m = module(indoc! {"
            Newspeak3
            'X'
            class C = Object new () ()
        "});
        let sc = m.root_class.superclass.as_ref().unwrap();
        assert_eq!(sc.receiver.name, "Object");
        assert_eq!(sc.messages.len(), 1);
    }

    #[test]
    fn slots_methods_and_nested_class() {
        // Comment + `|..|` slot section + init exprs live in G1; methods and
        // nested classes live in G2.
        let src = indoc! {"
            Newspeak3
            'X'
            class Outer usingPlatform: p = (
                (* a comment *)
                | x ::= 0. |
                x := 1.
            ) (
                public bump = ( x := x + 1 )
                class Inner = ( ) ( public ping = ( ^ 42 ) ) : ()
            )
        "};
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
        let src = indoc! {"
            Newspeak3
            'X'
            class C = () () : ( public default = ( ^ C new ) )
        "};
        let m = module(src);
        assert_eq!(m.root_class.class_side.as_ref().unwrap().methods.len(), 1);
    }

    #[test]
    fn trailing_garbage_is_an_error() {
        assert!(crate::parse(indoc! {"
            Newspeak3
            'X'
            class C = () ()
            extra
        "})
        .is_err());
    }

    // Malformed G1 input must terminate with a diagnostic, not hang:
    // `parse_expression` can return a recovery node without consuming a token,
    // so `parse_header_body`'s loop needs an explicit forward-progress guard.
    #[test]
    fn malformed_body_caret_is_error() {
        assert!(crate::parse(indoc! {"
            Newspeak3
            'X'
            class C = ( ^ 1 ) ()
        "})
        .is_err());
    }

    #[test]
    fn malformed_body_semi_is_error() {
        assert!(crate::parse(indoc! {"
            Newspeak3
            'X'
            class C = ( ; ) ()
        "})
        .is_err());
    }

    #[test]
    fn malformed_body_binsel_is_error() {
        assert!(crate::parse(indoc! {"
            Newspeak3
            'X'
            class C = ( + ) ()
        "})
        .is_err());
    }

    // A `|..|` slot section inside G2 is malformed (it belongs to G1).
    #[test]
    fn slot_section_in_g2_is_error() {
        assert!(crate::parse(indoc! {"
            Newspeak3
            'X'
            class C = () ( | x | ) : ()
        "})
        .is_err());
    }

    // --- corpus-shaped tests (real two-group grammar) ---

    #[test]
    fn two_group_with_class_side() {
        // Exact shape of corpus/modules/ADummyPush.ns.
        let src = indoc! {"
            Newspeak3
            'Newspeak'
            class ADummyPush new = Object new (
            ) (
                foo = (
                    ^99
                )
            ) : ()
        "};
        let m = module(src);
        assert_eq!(m.root_class.instance_side.methods.len(), 1);
        assert!(m.root_class.class_side.is_some());
    }

    #[test]
    fn nested_class_two_group() {
        let src = indoc! {"
            Newspeak3
            'X'
            class C = () ( class Inner = ( ) ( m = () ) : () ) : ()
        "};
        let m = module(src);
        assert_eq!(m.root_class.instance_side.nested_classes.len(), 1);
        assert_eq!(
            m.root_class.instance_side.nested_classes[0]
                .instance_side
                .methods
                .len(),
            1
        );
    }

    #[test]
    fn init_exprs_in_g1_only() {
        let m = module(indoc! {"
            Newspeak3
            'X'
            class C = ( x := 1. y := 2. ) ()
        "});
        let body = &m.root_class.instance_side;
        assert_eq!(body.init_exprs.len(), 2);
        assert!(body.methods.is_empty());
    }

    #[test]
    fn lazy_slot_in_g2() {
        let m = module(indoc! {"
            Newspeak3
            'X'
            class C = () ( lazy foo = bar baz. ) : ()
        "});
        let body = &m.root_class.instance_side;
        assert_eq!(body.slots.len(), 1);
        assert_eq!(body.slots[0].name.name, "foo");
        assert_eq!(body.slots[0].mutability, SlotMutability::Immutable);
    }

    #[test]
    fn lazy_slot_modifier_first_in_g2() {
        let m = module(indoc! {"
            Newspeak3
            'X'
            class C = () ( public lazy foo ::= bar. ) : ()
        "});
        let body = &m.root_class.instance_side;
        assert_eq!(body.slots.len(), 1);
        assert_eq!(body.slots[0].access, Some(AccessModifier::Public));
        assert_eq!(body.slots[0].mutability, SlotMutability::MutableInit);
    }

    // A method literally named `lazy` must not be mistaken for a lazy slot.
    #[test]
    fn method_named_lazy_is_not_a_slot() {
        let m = module(indoc! {"
            Newspeak3
            'X'
            class C = () ( lazy = ( ^ 1 ) ) : ()
        "});
        let body = &m.root_class.instance_side;
        assert!(body.slots.is_empty());
        assert_eq!(body.methods.len(), 1);
    }

    // --- I2: a leading string in the class header body is an init expression,
    // not a class comment. ---
    #[test]
    fn leading_string_in_header_is_an_init_expr_not_a_comment() {
        let m = module(indoc! {"
            Newspeak3
            'X'
            class C = ( 'hello' printLine. ) ()
        "});
        assert_eq!(m.root_class.class_comment, None);
        let body = &m.root_class.instance_side;
        assert_eq!(body.init_exprs.len(), 1);
        let Stmt::Expr(Expr::Send(s)) = &body.init_exprs[0] else {
            panic!("{:?}", body.init_exprs[0])
        };
        assert_eq!(s.message.selector, Selector::Unary("printLine".into()));
        assert!(
            matches!(s.receiver, Expr::Literal(Literal::String { .. })),
            "{:?}",
            s.receiver
        );
    }

    // The block-comment path is untouched: `(* ... *)` in G1 still becomes the
    // class comment.
    #[test]
    fn block_comment_in_header_is_still_the_class_comment() {
        let m = module(indoc! {"
            Newspeak3
            'X'
            class C = ( (* a comment *) ) ()
        "});
        assert_eq!(m.root_class.class_comment.as_deref(), Some(" a comment "));
    }

    // --- C1: recursion-depth guard. None of these may abort the process. ---
    #[test]
    fn deeply_nested_open_parens_do_not_overflow() {
        assert!(crate::parse(&"(".repeat(10_000)).is_err());
    }

    #[test]
    fn deeply_nested_open_brackets_do_not_overflow() {
        assert!(crate::parse(&"[".repeat(10_000)).is_err());
    }

    #[test]
    fn deeply_nested_but_closed_reports_depth_diagnostic() {
        let src = format!("{}x{}", "(".repeat(5000), ")".repeat(5000));
        let Err(diags) = crate::parse(&src) else {
            panic!("expected Err")
        };
        assert!(diags
            .iter()
            .any(|d| d.message.contains("nested too deeply")));
    }

    #[test]
    fn deeply_nested_classes_do_not_overflow() {
        let src = "class C = () (".repeat(5000);
        assert!(crate::parse(&src).is_err());
    }

    #[test]
    fn shallow_nesting_still_parses_clean() {
        let m = module(indoc! {"
            Newspeak3
            'X'
            class C = ( ((((x)))). ) ()
        "});
        assert_eq!(m.root_class.instance_side.init_exprs.len(), 1);
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

    // --- Declaration-position `<Type>` annotations & friends ---

    #[test]
    fn keyword_method_param_type_annotation() {
        let m = method("at: k <Integer> put: v <Object> = ( ^ nil )");
        let MessagePattern::Keyword { parts } = &m.pattern else {
            panic!("{:?}", m.pattern)
        };
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].1.name, "k");
        assert_eq!(parts[1].1.name, "v");
    }

    #[test]
    fn binary_method_param_type_annotation() {
        let m = method("+ other <Foo> = ( ^ self )");
        let MessagePattern::Binary { selector, param } = &m.pattern else {
            panic!()
        };
        assert_eq!(selector.name, "+");
        assert_eq!(param.name, "other");
    }

    #[test]
    fn nested_type_annotation_is_one_unit() {
        // `<Tuple[String]>` / `<Map[K, V]>` / `<{String. Namespace}>` /
        // `<[:E | Object]>` are each consumed whole.
        for src in [
            "get: k <Map[K, V]> = ( ^ nil )",
            "get: k <Tuple[String]> = ( ^ nil )",
            "on: b <[:E | Object]> = ( ^ nil )",
            "with: p <{String. Namespace}> = ( ^ nil )",
        ] {
            let m = method(src);
            assert!(matches!(m.pattern, MessagePattern::Keyword { .. }));
        }
    }

    #[test]
    fn method_return_type_annotation() {
        let m = method("storageKey ^<String> = ( ^ 'k' )");
        assert!(matches!(m.pattern, MessagePattern::Unary { .. }));
        let m = method("seed: v <Value> ^<FarReference[Value]> = ( ^ nil )");
        assert!(matches!(m.pattern, MessagePattern::Keyword { .. }));
    }

    #[test]
    fn equals_and_bar_are_binary_operator_method_names() {
        for (src, sel) in [
            ("= other = ( ^ true )", "="),
            ("< other = ( ^ true )", "<"),
            ("| p <Parser> ^<Parser> = ( ^ self )", "|"),
        ] {
            let m = method(src);
            let MessagePattern::Binary { selector, .. } = &m.pattern else {
                panic!("{src}: {:?}", m.pattern)
            };
            assert_eq!(selector.name, sel);
        }
    }

    #[test]
    fn selector_type_parameters_and_where_clause() {
        let m = method("List[E] = ( ^ MutableArrayList[E] )");
        assert!(matches!(m.pattern, MessagePattern::Unary { .. }));
        let m = method("reduce: f <[:RE :RE | RE]> ^<RE> where { C <: RE :: nil } = ( ^ nil )");
        assert!(matches!(m.pattern, MessagePattern::Keyword { .. }));
    }

    #[test]
    fn unterminated_type_annotation_recovers_without_panic() {
        // `skip_type_annotation` must push a diagnostic and stop, not run away
        // or panic, on a malformed `<...>`.
        let tokens = crate::lex("foo: x <Bad = ( ^ nil )").unwrap();
        let mut p = Parser::new("foo: x <Bad = ( ^ nil )", tokens);
        let _ = p.parse_method_decl_for_test();
        assert!(!p.diags_is_empty());
    }

    // `codeBody` temporaries: `temporaries = slotDecls` (grammar :148-150).

    #[test]
    fn empty_double_bar_temps() {
        // Lone `||` is the degenerate empty simultaneous temp list; the body
        // after it must still parse and no diagnostic is emitted.
        let m = method("foo = ( || ^ 1 )");
        assert!(m.body.temps.is_empty());
        assert!(matches!(m.body.stmts.as_slice(), [Stmt::Return(_)]));
    }

    #[test]
    fn empty_seq_bar_temps() {
        let m = method("foo = ( | | ^ 1 )");
        assert!(m.body.temps.is_empty());
        assert_eq!(m.body.stmts.len(), 1);
    }

    // A well-formed (dot-terminated) single initialized local: the closing `|`
    // is the section terminator, not a binary operator.
    #[test]
    fn single_initialized_local_temp_with_dot() {
        let m = method("foo = ( | x = 1. | ^ x )");
        assert_eq!(
            m.body
                .temps
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            ["x"]
        );
        assert!(matches!(m.body.stmts.as_slice(), [Stmt::Return(_)]));
    }

    #[test]
    fn plain_seq_temps() {
        let m = method("foo = ( | a b | ^ a )");
        assert_eq!(
            m.body
                .temps
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn initialized_local_slot_temps() {
        // The `| x = expr. |` full slot-decl form: initializer is parsed (body
        // stays in sync) but only the name is retained in Milestone 1.
        let m = method("foo = ( | x = 5. y ::= bar baz. | ^ x )");
        assert_eq!(
            m.body
                .temps
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            ["x", "y"]
        );
        assert!(matches!(m.body.stmts.as_slice(), [Stmt::Return(_)]));
    }
}
