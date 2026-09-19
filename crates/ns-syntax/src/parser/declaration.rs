//! Declaration-level parsing: classes, methods, slots and code bodies.

use super::Parser;
use crate::ast::*;
use crate::token::TokenKind;

impl<'a> Parser<'a> {
    /// Optional `| temporaries |` then `Dot`-separated statements, where the
    /// last statement may be `^ expression`. A trailing `Dot` is allowed. Stops
    /// at `RightBracket` / `RightParenthesis` / `EndOfInput`.
    pub(crate) fn parse_code_body(&mut self) -> CodeBody {
        let start = self.current().span;
        let temporaries = self.parse_temporaries();
        let mut statements = Vec::new();
        loop {
            match self.peek() {
                TokenKind::RightBracket | TokenKind::RightParenthesis | TokenKind::EndOfInput => {
                    break
                }
                TokenKind::Caret => {
                    self.bump(); // `^`
                    let expression = self.parse_expression();
                    statements.push(Statement::Return(expression));
                    self.eat(TokenKind::Dot); // optional trailing dot
                    break; // nothing after a return
                }
                TokenKind::Dot => {
                    self.bump(); // stray dot
                }
                _ => {
                    let expression = self.parse_expression();
                    statements.push(Statement::Expression(expression));
                    if !self.eat(TokenKind::Dot) {
                        break;
                    }
                }
            }
        }
        let end = self.current().span;
        CodeBody {
            temporaries,
            statements,
            span: start.merge(end),
        }
    }

    /// `codeBody` temporaries. Per the grammar
    /// (`corpus/reference/NewspeakGrammar.ns:148-150`), `temporaries =
    /// slotDecls`, i.e. a temporary list has the full slot-declaration form,
    /// including initialized locals `| x = expression. |`. `CodeBody::temporaries`
    /// keeps the full declaration, so the runtime can initialize each slot in
    /// declaration order (§3.6) and tell an immutable one from a mutable one;
    /// `lazy` is still dropped, as it is for class slots. A code body never
    /// legitimately starts with a binary `|` send (there is no left operand),
    /// so a leading `Bar` / `DoubleBar` is unambiguously a temporary section.
    fn parse_temporaries(&mut self) -> Vec<SlotDeclaration> {
        // The lexer emits `||` as one `DoubleBar` token. In code-body position
        // that lone token is the degenerate empty simultaneous temporary list —
        // short-circuit it before the `parse_slot_declarations` handoff, which
        // would otherwise consume `||` and then scan the rest of the body
        // hunting a closing `||` that never comes.
        if self.at(TokenKind::DoubleBar) {
            self.bump();
            return Vec::new();
        }
        if !self.at(TokenKind::Bar) {
            return Vec::new();
        }
        self.parse_slot_declarations()
    }

    pub(crate) fn parse_access_modifier(&mut self) -> Option<AccessModifier> {
        match self.peek() {
            TokenKind::Public => {
                self.bump();
                Some(AccessModifier::Public)
            }
            TokenKind::Protected => {
                self.bump();
                Some(AccessModifier::Protected)
            }
            TokenKind::Private => {
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
            TokenKind::BinarySelector
                | TokenKind::Equals
                | TokenKind::LessThan
                | TokenKind::GreaterThan
                | TokenKind::Bar
        ) && self.peek2() == TokenKind::Identifier
    }

    pub(crate) fn parse_message_pattern(&mut self) -> MessagePattern {
        // Declarations may carry `<Type>` annotations after each parameter name,
        // and a `^<Type>` return annotation before the `=`; Milestone 1 discards
        // them via `skip_type_annotation` / `skip_return_type_annotation`.
        let pattern = if self.at_binary_selector_pattern() {
            let selector = self.identifier_from_bump();
            let parameter = self.identifier();
            self.skip_type_annotation();
            MessagePattern::Binary {
                selector,
                parameter,
            }
        } else {
            match self.peek() {
                TokenKind::Keyword | TokenKind::SetterKeyword => {
                    let mut parts = Vec::new();
                    while self.at(TokenKind::Keyword) || self.at(TokenKind::SetterKeyword) {
                        let keyword = self.bump();
                        let key = Identifier {
                            name: self.text(keyword).trim_end_matches(':').to_string(),
                            span: keyword.span,
                        };
                        let parameter = self.identifier();
                        self.skip_type_annotation();
                        parts.push((key, parameter));
                    }
                    MessagePattern::Keyword { parts }
                }
                _ => {
                    let selector = self.identifier();
                    // `selectorTypeParameters` (`NewspeakGrammar.ns:446`):
                    // `public List[E] = (...)` — type parameters on the
                    // selector itself. Dropped in Milestone 1.
                    self.skip_type_arguments();
                    self.skip_type_annotation();
                    MessagePattern::Unary { selector }
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
        if self.at(TokenKind::Caret) && self.peek2() == TokenKind::LessThan {
            self.bump(); // `^`
            self.skip_type_annotation();
        }
    }

    pub(crate) fn parse_method_declaration(&mut self) -> MethodDeclaration {
        let start = self.current().span;
        let access = self.parse_access_modifier();
        let pattern = self.parse_message_pattern();
        self.expect(TokenKind::Equals);
        self.expect(TokenKind::LeftParenthesis);
        let body = self.parse_code_body();
        let end = self.current().span;
        self.expect(TokenKind::RightParenthesis);
        MethodDeclaration {
            access,
            pattern,
            body,
            span: start.merge(end),
        }
    }

    pub(crate) fn parse_slot_declarations(&mut self) -> Vec<SlotDeclaration> {
        let simultaneous = self.at(TokenKind::DoubleBar);
        let close = if simultaneous {
            TokenKind::DoubleBar
        } else {
            TokenKind::Bar
        };
        if !self.eat(close) {
            return Vec::new();
        }
        let mut slots = Vec::new();
        while !self.at(close) && !self.at(TokenKind::EndOfInput) {
            let start = self.current().span;
            let access = self.parse_access_modifier();
            // A slot / temporary may be named `class` (`| class |` locals in
            // the mirror libraries); `class` lexes as `TokenKind::Class`.
            if !self.at(TokenKind::Identifier) && !self.at(TokenKind::Class) {
                // recovery: skip to next '.' or close
                let token = self.bump();
                self.push_error(token.span, "expected a slot name");
                self.recover_to_dot(close);
                continue;
            }
            let name = self.identifier();
            // `slotName <Type>` — the annotation is dropped in Milestone 1.
            self.skip_type_annotation();
            let (mutability, initializer) = match self.peek() {
                TokenKind::Equals => {
                    self.bump();
                    (SlotMutability::Immutable, Some(self.parse_expression()))
                }
                TokenKind::ColonColonEquals => {
                    self.bump();
                    (
                        SlotMutability::MutableInitialized,
                        Some(self.parse_expression()),
                    )
                }
                _ => (SlotMutability::MutableBare, None),
            };
            let end = self.current().span;
            self.eat(TokenKind::Dot);
            slots.push(SlotDeclaration {
                access,
                name,
                mutability,
                initializer,
                span: start.merge(end),
            });
        }
        self.expect(close);
        slots
    }

    fn recover_to_dot(&mut self, close: TokenKind) {
        while !self.at(TokenKind::Dot) && !self.at(close) && !self.at(TokenKind::EndOfInput) {
            self.bump();
        }
        self.eat(TokenKind::Dot);
    }

    fn identifier_from_bump(&mut self) -> Identifier {
        let token = self.bump();
        Identifier {
            name: self.text(token).to_string(),
            span: token.span,
        }
    }
}

impl<'a> Parser<'a> {
    /// Top level: `languageId category classDecl EndOfInput`.
    ///
    /// `languageId` is the first `Identifier` token (`Newspeak3`); `category`
    /// is the first `String` literal. Missing pieces and trailing tokens are
    /// reported but parsing continues (recovery).
    pub(crate) fn parse_module(&mut self) -> Module {
        let start = self.current().span;
        let language_id = if self.at(TokenKind::Identifier) {
            let token = self.bump();
            self.text(token).to_string()
        } else {
            self.push_error(
                self.current().span,
                "expected a language id (e.g. `Newspeak3`)",
            );
            String::new()
        };
        // DEVIATION (grammar line 202: `classCategory = string opt`): the
        // grammar makes the category string optional; we require it and emit a
        // diagnostic when it is absent. Every corpus module carries one;
        // revisit in Milestone 2.
        let category = if self.at(TokenKind::String) {
            let token = self.bump();
            string_lexeme_body(self.text(token))
        } else {
            self.push_error(self.current().span, "expected a category string");
            String::new()
        };
        let root_class = self.parse_class_declaration();
        let end = self.current().span;
        if !self.at(TokenKind::EndOfInput) {
            self.push_error(
                self.current().span,
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
    ///   initializer expressions. No methods, no nested classes.
    /// - **G2** (`sideDecl` body): nested classes, methods, and lazy slot declarations.
    /// - **G3** (`classSideDecl` body): methods only.
    ///
    /// The AST keeps a single `instance_side: ClassBody`: G1's slots (plus any
    /// G2 lazy slot declarations) land in `slots`, G1's initializer expressions in
    /// `initializer_expressions`, and G2's methods / nested classes in
    /// `methods` / `nested_classes`. `class_side` is G3.
    pub(crate) fn parse_class_declaration(&mut self) -> ClassDeclaration {
        let start = self.current().span;
        let access = self.parse_access_modifier();
        self.expect(TokenKind::Class);
        let name = self.identifier();
        // `class List[E] ...` — generic type-parameter declaration
        // (`NewspeakGrammar.ns:465`); dropped in Milestone 1.
        self.skip_type_arguments();
        let factory = if self.at(TokenKind::Equals) {
            None
        } else {
            Some(self.parse_message_pattern())
        };
        self.expect(TokenKind::Equals);
        // DEVIATION (grammar lines 49, 195, 214-221): mixin application
        // `class C = Super <: Mixin (...) (...)` (the `inverseMixinOperator`
        // `<:`, `mixinApplication` / `mixinAppSuffix`) is unimplemented for
        // Milestone 1 — no corpus file uses `<:` in a superclass clause.
        // `parse_superclass_clause` parses the receiver and its trailing
        // message sends only; a following `<:` is left for the caller to flag.
        let superclass = if self.at(TokenKind::LeftParenthesis) {
            None
        } else {
            Some(self.parse_superclass_clause())
        };

        // --- G1: classHeader body `( classComment? slotDecls? initExprs )` ---
        self.expect(TokenKind::LeftParenthesis);
        let class_comment = self.take_class_comment();
        let (header_slots, initializer_expressions, g1_span) = self.parse_header_body();
        let g1_end = self.current().span;
        self.expect(TokenKind::RightParenthesis);

        // --- G2: sideDecl body `( ( classDecl | lazySlotDecl | methodDecl )* )` ---
        // Corpus always has G2; being lenient when it is absent keeps a
        // degenerate `class C = ()` parseable.
        let (lazy_slots, methods, nested_classes, g2_end) = if self.at(TokenKind::LeftParenthesis) {
            self.bump(); // `(`
            let (lazy, methods, nested) = self.parse_side_body();
            let end = self.current().span;
            self.expect(TokenKind::RightParenthesis);
            (lazy, methods, nested, end)
        } else {
            (Vec::new(), Vec::new(), Vec::new(), g1_end)
        };

        let mut slots = header_slots;
        slots.extend(lazy_slots);
        let instance_side = ClassBody {
            slots,
            initializer_expressions,
            methods,
            nested_classes,
            span: g1_span.merge(g2_end),
        };

        // --- G3: classSideDecl body `: ( methodDecl* )` ---
        let class_side = if self.eat(TokenKind::Colon) {
            self.expect(TokenKind::LeftParenthesis);
            // A class-side comment is allowed but not retained.
            let _ = self.take_class_comment();
            let body = self.parse_class_side_body();
            self.expect(TokenKind::RightParenthesis);
            Some(body)
        } else {
            None
        };

        let end = class_side.as_ref().map(|body| body.span).unwrap_or(g2_end);
        ClassDeclaration {
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
    /// valid initializer expression, so that branch was removed.
    fn take_class_comment(&mut self) -> Option<String> {
        let previous_end = self.tokens.get(self.position.checked_sub(1)?)?.span.end as usize;
        let current_start = self.current().span.start as usize;
        let gap = self.source.get(previous_end..current_start)?.trim();
        let inner = gap.strip_prefix("(*")?.strip_suffix("*)")?;
        Some(inner.to_string())
    }

    /// `superclassPrefix? Identifier message?` — the receiver identifier
    /// followed by zero or more unary/binary/keyword sends (`Object new`,
    /// `platform collections List`, `superclass subclassMixin: m`).
    pub(crate) fn parse_superclass_clause(&mut self) -> SuperclassClause {
        let start = self.current().span;
        let prefix = match self.peek() {
            TokenKind::Identifier => match self.text(self.current()) {
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
                    Some(SuperclassPrefix::Outer(self.identifier()))
                }
                _ => None,
            },
            _ => None,
        };
        let receiver = self.identifier();
        // `Collection[E] new` — type arguments on the superclass receiver.
        self.skip_type_arguments();
        let mut messages = Vec::new();
        self.parse_trailing_sends(&mut messages);
        let end = messages
            .last()
            .map(|module| module.span)
            .unwrap_or(receiver.span);
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
        while (self.at(TokenKind::Identifier) || self.at(TokenKind::Class))
            && !self.is_reserved_identifier(self.current())
        {
            let selector_token = self.bump();
            out.push(MessageSend {
                selector: Selector::Unary(self.text(selector_token).to_string()),
                arguments: Vec::new(),
                span: selector_token.span,
            });
        }
        while self.at(TokenKind::BinarySelector) {
            let operator = self.bump();
            let argument = self.parse_unary_message();
            out.push(MessageSend {
                selector: Selector::Binary(self.text(operator).to_string()),
                span: operator
                    .span
                    .merge(super::expression::expression_span(&argument)),
                arguments: vec![argument],
            });
        }
        if self.at(TokenKind::Keyword) {
            let mut selector = String::new();
            let mut arguments = Vec::new();
            let start = self.current().span;
            let mut end = start;
            while self.at(TokenKind::Keyword) {
                let keyword = self.bump();
                selector.push_str(self.text(keyword).trim_end_matches(':'));
                selector.push(':');
                let argument = self.parse_binary_message();
                end = super::expression::expression_span(&argument);
                arguments.push(argument);
            }
            out.push(MessageSend {
                selector: Selector::Keyword(selector),
                arguments,
                span: start.merge(end),
            });
        }
    }

    /// **G1** — `classHeader` body (`NewspeakGrammar.ns:208`):
    /// `classComment? , slotDecls? , initExprs`. The class comment is consumed
    /// by the caller via [`Parser::take_class_comment`] right after the `(`.
    /// Here: an optional single slot section (`| .. |` or `|| .. ||`), then
    /// zero or more `Dot`-separated/terminated initializer expressions, until `)` or
    /// end of input. Methods, nested classes and a second slot section are NOT
    /// accepted here — they belong to G2.
    fn parse_header_body(&mut self) -> (Vec<SlotDeclaration>, Vec<Statement>, crate::span::Span) {
        let start = self.current().span;
        let slots = if self.at(TokenKind::Bar) || self.at(TokenKind::DoubleBar) {
            self.parse_slot_declarations()
        } else {
            Vec::new()
        };
        let mut initializer_expressions = Vec::new();
        loop {
            match self.peek() {
                TokenKind::RightParenthesis | TokenKind::EndOfInput => break,
                TokenKind::Dot => {
                    self.bump();
                }
                _ => {
                    let before = self.position;
                    let expression = self.parse_expression();
                    self.eat(TokenKind::Dot);
                    if self.position == before {
                        // `parse_expression` can return a recovery node without
                        // consuming a token (a stray `^ ; + = ]` etc). Force
                        // progress and drop the bogus node.
                        let token = self.bump();
                        self.push_error(token.span, "expected an init expression or `)`");
                    } else {
                        initializer_expressions.push(Statement::Expression(expression));
                    }
                }
            }
        }
        let end = self.current().span;
        (slots, initializer_expressions, start.merge(end))
    }

    /// **G2** — `sideDecl` body (`NewspeakGrammar.ns:189`):
    /// `( classDecl | lazySlotDecl | methodDecl )*`. Returns the lazy slot
    /// declarations (merged into `ClassBody::slots` by the caller), the methods, and
    /// the nested classes. A `| .. |` slot section, a bare initializer expression or a
    /// class comment here is malformed (those belong to G1).
    fn parse_side_body(
        &mut self,
    ) -> (
        Vec<SlotDeclaration>,
        Vec<MethodDeclaration>,
        Vec<ClassDeclaration>,
    ) {
        let mut lazy_slots = Vec::new();
        let mut methods = Vec::new();
        let mut nested_classes = Vec::new();
        loop {
            match self.peek() {
                TokenKind::RightParenthesis | TokenKind::EndOfInput => break,
                _ => {}
            }
            let before = self.position;
            match self.peek() {
                TokenKind::Dot => {
                    self.bump();
                }
                // A `| parameter ... = (...)` binary-operator method named `|`
                // is valid here; only a genuine `|...|` slot section is not.
                TokenKind::Bar | TokenKind::DoubleBar if !self.looks_like_method() => {
                    let token = self.bump();
                    self.push_error(
                        token.span,
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
                        let token = self.current();
                        self.push_error(token.span, "class nesting too deep");
                        self.skip_to_structural();
                        break;
                    }
                    nested_classes.push(self.parse_class_declaration());
                    self.leave();
                }
                _ if self.at_lazy_slot() => {
                    lazy_slots.push(self.parse_lazy_slot_declaration());
                }
                _ if self.looks_like_method() => {
                    methods.push(self.parse_method_declaration());
                }
                _ => {
                    let token = self.bump();
                    self.push_error(
                        token.span,
                        "expected a method, nested class, or lazy slot declaration",
                    );
                }
            }
            if self.position == before {
                // Forward-progress guard: nothing above consumed a token.
                let token = self.bump();
                self.push_error(token.span, "unexpected token in class body");
            }
        }
        (lazy_slots, methods, nested_classes)
    }

    /// **G3** — `classSideDecl` body (`NewspeakGrammar.ns:200`):
    /// `methodDecl*`. Methods only; slots / initializer expressions / nested classes stay
    /// empty.
    fn parse_class_side_body(&mut self) -> ClassBody {
        let start = self.current().span;
        let mut methods = Vec::new();
        loop {
            match self.peek() {
                TokenKind::RightParenthesis | TokenKind::EndOfInput => break,
                _ => {}
            }
            let before = self.position;
            match self.peek() {
                TokenKind::Dot => {
                    self.bump();
                }
                _ if self.looks_like_method() => {
                    methods.push(self.parse_method_declaration());
                }
                _ => {
                    let token = self.bump();
                    self.push_error(token.span, "expected a method declaration");
                }
            }
            if self.position == before {
                let token = self.bump();
                self.push_error(token.span, "unexpected token on the class side");
            }
        }
        let end = self.current().span;
        ClassBody {
            slots: Vec::new(),
            initializer_expressions: Vec::new(),
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
        if self.peek() == TokenKind::Class {
            return self.peek2() == TokenKind::Identifier;
        }
        if matches!(
            self.peek(),
            TokenKind::Public | TokenKind::Protected | TokenKind::Private
        ) && self.peek2() == TokenKind::Class
        {
            return self.tokens.get(self.position + 2).map(|token| token.kind)
                == Some(TokenKind::Identifier);
        }
        false
    }

    /// `lazy` lexes as an `Identifier` with text `"lazy"`. A lazy slot declaration is
    /// either `lazy accessModifier? slotName ...` (lazy-first) or
    /// `accessModifier lazy slotName ...` (modifier-first) —
    /// `NewspeakGrammar.ns:183-186`.
    fn at_lazy_slot(&self) -> bool {
        if self.at(TokenKind::Identifier) && self.text(self.current()) == "lazy" {
            // Distinguish from a method literally named `lazy` (`lazy = (...)`).
            return matches!(
                self.peek2(),
                TokenKind::Public
                    | TokenKind::Protected
                    | TokenKind::Private
                    | TokenKind::Identifier
            );
        }
        if matches!(
            self.peek(),
            TokenKind::Public | TokenKind::Protected | TokenKind::Private
        ) {
            if let Some(next_token) = self.tokens.get(self.position + 1) {
                return next_token.kind == TokenKind::Identifier
                    && self.text(*next_token) == "lazy";
            }
        }
        false
    }

    /// Parse a lazy slot declaration. Milestone 1's AST has no "lazy" flag, so
    /// the laziness is dropped and the slot is recorded as an ordinary
    /// [`SlotDeclaration`] (mutability from `=` vs `::=`). Both grammar
    /// orderings are accepted.
    fn parse_lazy_slot_declaration(&mut self) -> SlotDeclaration {
        let start = self.current().span;
        let access = if self.at(TokenKind::Identifier) && self.text(self.current()) == "lazy" {
            self.bump(); // `lazy`
            self.parse_access_modifier()
        } else {
            let modifier = self.parse_access_modifier();
            if self.at(TokenKind::Identifier) && self.text(self.current()) == "lazy" {
                self.bump(); // `lazy`
            }
            modifier
        };
        let name = self.identifier();
        self.skip_type_annotation();
        let mutability = if self.eat(TokenKind::ColonColonEquals) {
            SlotMutability::MutableInitialized
        } else {
            self.expect(TokenKind::Equals);
            SlotMutability::Immutable
        };
        let initializer = Some(self.parse_expression());
        let end = self.current().span;
        self.eat(TokenKind::Dot);
        SlotDeclaration {
            access,
            name,
            mutability,
            initializer,
            span: start.merge(end),
        }
    }

    /// Speculative lookahead: does the cursor sit at `accessModifier?
    /// messagePattern '=' '('` (a method), rather than an initializer expression?
    fn looks_like_method(&mut self) -> bool {
        let checkpoint = self.checkpoint();
        self.parse_access_modifier();
        let _ = self.try_parse_message_pattern();
        let is_method = self.at(TokenKind::Equals)
            && self.tokens.get(self.position + 1).map(|token| token.kind)
                == Some(TokenKind::LeftParenthesis);
        self.restore(checkpoint);
        is_method
    }

    fn try_parse_message_pattern(&mut self) -> Option<()> {
        // Mirror `parse_message_pattern`'s `<Type>` / `^<Type>` skipping and its
        // `= < >`-as-binary-selector handling so the speculative
        // `looks_like_method` lookahead lands on `=` `(`, not `<` / `^`.
        let matched = if self.at_binary_selector_pattern() {
            self.bump(); // selector
            self.bump(); // parameter identifier
            self.skip_type_annotation();
            Some(())
        } else {
            match self.peek() {
                TokenKind::Keyword | TokenKind::SetterKeyword => {
                    while self.at(TokenKind::Keyword) || self.at(TokenKind::SetterKeyword) {
                        self.bump();
                        if self.at(TokenKind::Identifier) {
                            self.bump();
                        } else {
                            return None;
                        }
                        self.skip_type_annotation();
                    }
                    Some(())
                }
                TokenKind::Identifier | TokenKind::Class => {
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
    pub(crate) fn parse_method_declaration_for_test(&mut self) -> MethodDeclaration {
        self.parse_method_declaration()
    }

    pub(crate) fn parse_slot_declarations_for_test(&mut self) -> Vec<SlotDeclaration> {
        self.parse_slot_declarations()
    }

    pub(crate) fn parse_module_for_test(&mut self) -> Module {
        self.parse_module()
    }

    pub(crate) fn diagnostics_is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub(crate) fn debug_diagnostics(&self) -> String {
        format!("{:?}", self.diagnostics)
    }
}

#[cfg(test)]
mod slot_tests {
    use super::*;
    use crate::parser::Parser;

    fn slots(source: &str) -> Vec<SlotDeclaration> {
        let tokens = crate::lex(source).unwrap();
        let mut parser = Parser::new(source, tokens);
        let slots = parser.parse_slot_declarations_for_test();
        assert!(
            parser.diagnostics_is_empty(),
            "{:?}",
            parser.debug_diagnostics()
        );
        slots
    }

    #[test]
    fn immutable_and_mutable_slots() {
        let slots = slots("| private List = platform collections List. count ::= 0. name. |");
        assert_eq!(slots.len(), 3);
        assert_eq!(slots[0].access, Some(AccessModifier::Private));
        assert_eq!(slots[0].mutability, SlotMutability::Immutable);
        assert_eq!(slots[1].mutability, SlotMutability::MutableInitialized);
        assert_eq!(slots[2].mutability, SlotMutability::MutableBare);
    }

    #[test]
    fn empty_section() {
        assert!(slots("| |").is_empty());
    }

    #[test]
    fn simultaneous_section() {
        let slots = slots("|| a = 1. b = 2. ||");
        assert_eq!(slots.len(), 2);
    }

    // --- Slot type annotations, and `class` as a name ---

    #[test]
    fn slot_type_annotation() {
        let slots = slots("| public foo <Bar> = 1. x <Baz[Y]> ::= 2. n <Q>. |");
        assert_eq!(slots.len(), 3);
        assert_eq!(slots[0].name.name, "foo");
        assert_eq!(slots[0].mutability, SlotMutability::Immutable);
        assert_eq!(slots[1].mutability, SlotMutability::MutableInitialized);
        assert_eq!(slots[2].mutability, SlotMutability::MutableBare);
    }

    #[test]
    fn slot_named_class() {
        // `| class |` locals appear in the mirror libraries.
        let slots = slots("| class |");
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].name.name, "class");
    }
}

#[cfg(test)]
mod class_tests {
    use super::*;
    use indoc::indoc;

    fn module(source: &str) -> Module {
        match crate::parse(source) {
            Ok(module) => module,
            Err(diagnostics) => panic!("parse errors: {diagnostics:#?}"),
        }
    }

    // Parse a module keeping the (possibly partial) AST *and* the diagnostics,
    // for exercising error recovery.
    fn module_with_diagnostics(source: &str) -> (Module, bool) {
        let tokens = crate::lex(source).unwrap();
        let mut parser = super::Parser::new(source, tokens);
        let module = parser.parse_module_for_test();
        let clean = parser.diagnostics_is_empty();
        (module, clean)
    }

    // A dot-less final slot initializer (invalid — grammar :165 makes the
    // trailing `.` mandatory) must not have its section-closing `|` eaten as
    // a binary `|` operator, which cascades into and swallows the following
    // G2 declaration.
    #[test]
    fn dotless_final_slot_initializer_does_not_swallow_following_declaration() {
        let source = indoc! {"
            Newspeak3
            'X'
            class Outer = (
             | a = X new |
            ) (
             class Inner = () () : ()
            ) : ()
        "};
        let (module, _clean) = module_with_diagnostics(source);
        let body = &module.root_class.instance_side;
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
        let module = module(indoc! {"
            Newspeak3
            'X'
            class C = (
             | flag = a | b. |
            ) ()
        "});
        let initializer = module.root_class.instance_side.slots[0]
            .initializer
            .as_ref()
            .unwrap();
        let Expression::Send(send) = initializer else {
            panic!("{initializer:?}")
        };
        assert_eq!(send.message.selector, Selector::Binary("|".into()));
        let Expression::Identifier(argument) = &send.message.arguments[0] else {
            panic!()
        };
        assert_eq!(argument.name, "b");
    }

    #[test]
    fn minimal_module() {
        // Real grammar: G1 `()` then G2 `()` (both empty).
        let module = module(indoc! {"
            Newspeak3
            'App'
            class Empty usingPlatform: p = () ()
        "});
        assert_eq!(module.language_id, "Newspeak3");
        assert_eq!(module.category, "App");
        assert_eq!(module.root_class.name.name, "Empty");
        let MessagePattern::Keyword { parts } = &module.root_class.factory.as_ref().unwrap() else {
            panic!()
        };
        assert_eq!(parts[0].0.name, "usingPlatform");
        assert!(module.root_class.instance_side.slots.is_empty());
        assert!(module.root_class.instance_side.methods.is_empty());
    }

    #[test]
    fn superclass_clause_with_send() {
        let module = module(indoc! {"
            Newspeak3
            'X'
            class C = Object new () ()
        "});
        let superclass = module.root_class.superclass.as_ref().unwrap();
        assert_eq!(superclass.receiver.name, "Object");
        assert_eq!(superclass.messages.len(), 1);
    }

    #[test]
    fn slots_methods_and_nested_class() {
        // Comment + `|..|` slot section + initializer expressions live in G1; methods and
        // nested classes live in G2.
        let source = indoc! {"
            Newspeak3
            'X'
            class Outer usingPlatform: p = (
                (* a comment *)
                | x ::= 0. |
                x:: 1.
            ) (
                public bump = ( x:: x + 1 )
                class Inner = ( ) ( public ping = ( ^ 42 ) ) : ()
            )
        "};
        let module = module(source);
        let body = &module.root_class.instance_side;
        assert_eq!(body.slots.len(), 1);
        assert_eq!(body.initializer_expressions.len(), 1);
        assert_eq!(body.methods.len(), 1);
        assert_eq!(body.nested_classes.len(), 1);
        assert_eq!(
            module.root_class.class_comment.as_deref(),
            Some(" a comment ")
        );
    }

    #[test]
    fn class_side_block() {
        let source = indoc! {"
            Newspeak3
            'X'
            class C = () () : ( public default = ( ^ C new ) )
        "};
        let module = module(source);
        assert_eq!(
            module.root_class.class_side.as_ref().unwrap().methods.len(),
            1
        );
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
    fn malformed_body_semicolon_is_error() {
        assert!(crate::parse(indoc! {"
            Newspeak3
            'X'
            class C = ( ; ) ()
        "})
        .is_err());
    }

    #[test]
    fn malformed_body_binary_selector_is_error() {
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
        let source = indoc! {"
            Newspeak3
            'Newspeak'
            class ADummyPush new = Object new (
            ) (
                foo = (
                    ^99
                )
            ) : ()
        "};
        let module = module(source);
        assert_eq!(module.root_class.instance_side.methods.len(), 1);
        assert!(module.root_class.class_side.is_some());
    }

    #[test]
    fn nested_class_two_group() {
        let source = indoc! {"
            Newspeak3
            'X'
            class C = () ( class Inner = ( ) ( m = () ) : () ) : ()
        "};
        let module = module(source);
        assert_eq!(module.root_class.instance_side.nested_classes.len(), 1);
        assert_eq!(
            module.root_class.instance_side.nested_classes[0]
                .instance_side
                .methods
                .len(),
            1
        );
    }

    #[test]
    fn initializer_expressions_in_g1_only() {
        let module = module(indoc! {"
            Newspeak3
            'X'
            class C = ( x:: 1. y:: 2. ) ()
        "});
        let body = &module.root_class.instance_side;
        assert_eq!(body.initializer_expressions.len(), 2);
        assert!(body.methods.is_empty());
    }

    #[test]
    fn lazy_slot_in_g2() {
        let module = module(indoc! {"
            Newspeak3
            'X'
            class C = () ( lazy foo = bar baz. ) : ()
        "});
        let body = &module.root_class.instance_side;
        assert_eq!(body.slots.len(), 1);
        assert_eq!(body.slots[0].name.name, "foo");
        assert_eq!(body.slots[0].mutability, SlotMutability::Immutable);
    }

    #[test]
    fn lazy_slot_modifier_first_in_g2() {
        let module = module(indoc! {"
            Newspeak3
            'X'
            class C = () ( public lazy foo ::= bar. ) : ()
        "});
        let body = &module.root_class.instance_side;
        assert_eq!(body.slots.len(), 1);
        assert_eq!(body.slots[0].access, Some(AccessModifier::Public));
        assert_eq!(body.slots[0].mutability, SlotMutability::MutableInitialized);
    }

    // A method literally named `lazy` must not be mistaken for a lazy slot.
    #[test]
    fn method_named_lazy_is_not_a_slot() {
        let module = module(indoc! {"
            Newspeak3
            'X'
            class C = () ( lazy = ( ^ 1 ) ) : ()
        "});
        let body = &module.root_class.instance_side;
        assert!(body.slots.is_empty());
        assert_eq!(body.methods.len(), 1);
    }

    // --- I2: a leading string in the class header body is an initializer expression,
    // not a class comment. ---
    #[test]
    fn leading_string_in_header_is_an_initializer_expression_not_a_comment() {
        let module = module(indoc! {"
            Newspeak3
            'X'
            class C = ( 'hello' printLine. ) ()
        "});
        assert_eq!(module.root_class.class_comment, None);
        let body = &module.root_class.instance_side;
        assert_eq!(body.initializer_expressions.len(), 1);
        let Statement::Expression(Expression::Send(send)) = &body.initializer_expressions[0] else {
            panic!("{:?}", body.initializer_expressions[0])
        };
        assert_eq!(send.message.selector, Selector::Unary("printLine".into()));
        assert!(
            matches!(send.receiver, Expression::Literal(Literal::String { .. })),
            "{:?}",
            send.receiver
        );
    }

    // The block-comment path is untouched: `(* ... *)` in G1 still becomes the
    // class comment.
    #[test]
    fn block_comment_in_header_is_still_the_class_comment() {
        let module = module(indoc! {"
            Newspeak3
            'X'
            class C = ( (* a comment *) ) ()
        "});
        assert_eq!(
            module.root_class.class_comment.as_deref(),
            Some(" a comment ")
        );
    }

    // --- C1: recursion-depth guard. None of these may abort the process. ---
    #[test]
    fn deeply_nested_open_parentheses_do_not_overflow() {
        assert!(crate::parse(&"(".repeat(10_000)).is_err());
    }

    #[test]
    fn deeply_nested_open_brackets_do_not_overflow() {
        assert!(crate::parse(&"[".repeat(10_000)).is_err());
    }

    #[test]
    fn deeply_nested_but_closed_reports_depth_diagnostic() {
        let source = format!("{}x{}", "(".repeat(5000), ")".repeat(5000));
        let Err(diagnostics) = crate::parse(&source) else {
            panic!("expected Err")
        };
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("nested too deeply")));
    }

    #[test]
    fn deeply_nested_classes_do_not_overflow() {
        let source = "class C = () (".repeat(5000);
        assert!(crate::parse(&source).is_err());
    }

    #[test]
    fn shallow_nesting_still_parses_clean() {
        let module = module(indoc! {"
            Newspeak3
            'X'
            class C = ( ((((x)))). ) ()
        "});
        assert_eq!(
            module
                .root_class
                .instance_side
                .initializer_expressions
                .len(),
            1
        );
    }
}

#[cfg(test)]
mod method_tests {
    use super::*;
    use crate::parser::Parser;

    fn method(source: &str) -> MethodDeclaration {
        let tokens = crate::lex(source).unwrap();
        let mut parser = Parser::new(source, tokens);
        let method = parser.parse_method_declaration_for_test();
        assert!(
            parser.diagnostics_is_empty(),
            "{:?}",
            parser.debug_diagnostics()
        );
        method
    }

    #[test]
    fn unary_method() {
        let method = method("greeting = ( ^ 'hi' )");
        assert!(matches!(method.pattern, MessagePattern::Unary { .. }));
        assert_eq!(method.body.statements.len(), 1);
    }

    #[test]
    fn keyword_method_with_access() {
        let method = method("public at: k put: v = ( slot at: k put: v. ^ v )");
        let MessagePattern::Keyword { parts } = &method.pattern else {
            panic!()
        };
        assert_eq!(parts.len(), 2);
        assert_eq!(method.access, Some(AccessModifier::Public));
    }

    #[test]
    fn binary_method() {
        let method = method("+ other = ( ^ self addTo: other )");
        assert!(matches!(method.pattern, MessagePattern::Binary { .. }));
    }

    // --- Declaration-position `<Type>` annotations & friends ---

    #[test]
    fn keyword_method_parameter_type_annotation() {
        let method = method("at: k <Integer> put: v <Object> = ( ^ nil )");
        let MessagePattern::Keyword { parts } = &method.pattern else {
            panic!("{:?}", method.pattern)
        };
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].1.name, "k");
        assert_eq!(parts[1].1.name, "v");
    }

    #[test]
    fn binary_method_parameter_type_annotation() {
        let method = method("+ other <Foo> = ( ^ self )");
        let MessagePattern::Binary {
            selector,
            parameter,
        } = &method.pattern
        else {
            panic!()
        };
        assert_eq!(selector.name, "+");
        assert_eq!(parameter.name, "other");
    }

    #[test]
    fn nested_type_annotation_is_one_unit() {
        // `<Tuple[String]>` / `<Map[K, V]>` / `<{String. Namespace}>` /
        // `<[:E | Object]>` are each consumed whole.
        for source in [
            "get: k <Map[K, V]> = ( ^ nil )",
            "get: k <Tuple[String]> = ( ^ nil )",
            "on: b <[:E | Object]> = ( ^ nil )",
            "with: p <{String. Namespace}> = ( ^ nil )",
        ] {
            let method = method(source);
            assert!(matches!(method.pattern, MessagePattern::Keyword { .. }));
        }
    }

    #[test]
    fn method_return_type_annotation() {
        let storage_key = method("storageKey ^<String> = ( ^ 'k' )");
        assert!(matches!(storage_key.pattern, MessagePattern::Unary { .. }));
        let seed = method("seed: v <Value> ^<FarReference[Value]> = ( ^ nil )");
        assert!(matches!(seed.pattern, MessagePattern::Keyword { .. }));
    }

    #[test]
    fn equals_and_bar_are_binary_operator_method_names() {
        for (source, expected_selector) in [
            ("= other = ( ^ true )", "="),
            ("< other = ( ^ true )", "<"),
            ("| p <Parser> ^<Parser> = ( ^ self )", "|"),
        ] {
            let method = method(source);
            let MessagePattern::Binary { selector, .. } = &method.pattern else {
                panic!("{source}: {:?}", method.pattern)
            };
            assert_eq!(selector.name, expected_selector);
        }
    }

    #[test]
    fn selector_type_parameters_and_where_clause() {
        let list = method("List[E] = ( ^ MutableArrayList[E] )");
        assert!(matches!(list.pattern, MessagePattern::Unary { .. }));
        let reduce =
            method("reduce: f <[:RE :RE | RE]> ^<RE> where { C <: RE :: nil } = ( ^ nil )");
        assert!(matches!(reduce.pattern, MessagePattern::Keyword { .. }));
    }

    #[test]
    fn unterminated_type_annotation_recovers_without_panic() {
        // `skip_type_annotation` must push a diagnostic and stop, not run away
        // or panic, on a malformed `<...>`.
        let tokens = crate::lex("foo: x <Bad = ( ^ nil )").unwrap();
        let mut parser = Parser::new("foo: x <Bad = ( ^ nil )", tokens);
        let _ = parser.parse_method_declaration_for_test();
        assert!(!parser.diagnostics_is_empty());
    }

    // `codeBody` temporaries: `temporaries = slotDecls` (grammar :148-150).

    #[test]
    fn empty_double_bar_temporaries() {
        // Lone `||` is the degenerate empty simultaneous temporary list; the body
        // after it must still parse and no diagnostic is emitted.
        let method = method("foo = ( || ^ 1 )");
        assert!(method.body.temporaries.is_empty());
        assert!(matches!(
            method.body.statements.as_slice(),
            [Statement::Return(_)]
        ));
    }

    #[test]
    fn empty_sequence_bar_temporaries() {
        let method = method("foo = ( | | ^ 1 )");
        assert!(method.body.temporaries.is_empty());
        assert_eq!(method.body.statements.len(), 1);
    }

    // A well-formed (dot-terminated) single initialized local: the closing `|`
    // is the section terminator, not a binary operator.
    #[test]
    fn single_initialized_local_temporary_with_dot() {
        let method = method("foo = ( | x = 1. | ^ x )");
        assert_eq!(
            method
                .body
                .temporaries
                .iter()
                .map(|temporary| temporary.name.name.as_str())
                .collect::<Vec<_>>(),
            ["x"]
        );
        assert!(matches!(
            method.body.statements.as_slice(),
            [Statement::Return(_)]
        ));
    }

    #[test]
    fn plain_sequence_temporaries() {
        let method = method("foo = ( | a b | ^ a )");
        assert_eq!(
            method
                .body
                .temporaries
                .iter()
                .map(|temporary| temporary.name.name.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn initialized_local_slot_temporaries() {
        // The `| x = expression. |` full slot-declaration form: the initializer
        // is kept, along with the name.
        let method = method("foo = ( | x = 5. y ::= bar baz. | ^ x )");
        assert_eq!(
            method
                .body
                .temporaries
                .iter()
                .map(|temporary| temporary.name.name.as_str())
                .collect::<Vec<_>>(),
            ["x", "y"]
        );
        assert!(matches!(
            method.body.statements.as_slice(),
            [Statement::Return(_)]
        ));
    }

    #[test]
    fn temporaries_keep_their_full_declaration_including_mutability() {
        // `| x = 1. y ::= 2. z |`: three declarations, their mutabilities and
        // initializers kept, not just their names -- Task 3 reads `mutability`
        // to tell a setter-bearing local from one that declares no setter.
        let method = method("foo = ( | x = 1. y ::= 2. z | ^ x )");
        assert_eq!(
            method
                .body
                .temporaries
                .iter()
                .map(|temporary| (
                    temporary.name.name.as_str(),
                    temporary.mutability,
                    temporary.initializer.is_some()
                ))
                .collect::<Vec<_>>(),
            [
                ("x", SlotMutability::Immutable, true),
                ("y", SlotMutability::MutableInitialized, true),
                ("z", SlotMutability::MutableBare, false),
            ]
        );
    }
}
