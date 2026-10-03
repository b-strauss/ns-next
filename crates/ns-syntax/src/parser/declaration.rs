//! Declaration-level parsing: classes, methods, slots and code bodies.

use super::Parser;
use crate::ast::*;
use crate::token::{Token, TokenKind};

impl<'a> Parser<'a> {
    /// `codeBody → [temporaries] statements` (§5.1.8, and §7.2 for the return), where
    /// `statements → returnStatement | statementSequence | empty`,
    /// `statementSequence → expression [dot statements]` and
    /// `returnStatement → hat expression [dot]`: statements separated by
    /// single dots, a trailing one allowed, nothing after a return, and no
    /// dot that ends no statement. Stops at `]`, `)` or the end of input.
    pub(crate) fn parse_code_body(&mut self) -> CodeBody {
        let start = self.current().span;
        let (temporaries, temporaries_delimiters) = self.parse_temporaries();
        let mut statements = Vec::new();
        loop {
            match self.peek() {
                TokenKind::RightBracket | TokenKind::RightParenthesis | TokenKind::EndOfInput => {
                    break
                }
                TokenKind::Caret => {
                    let caret = self.bump();
                    let expression = self.parse_statement().unwrap_or_else(|| {
                        let token = self.current();
                        self.push_error(
                            token.span,
                            &format!("expected an expression, found {}", self.found(token)),
                        );
                        Expression::Literal(Literal::Nil { span: caret.span })
                    });
                    statements.push(Statement::Return(expression));
                    self.eat(TokenKind::Dot); // optional trailing dot

                    // Nothing follows a return (§7.2). What does is skipped to
                    // the body's end, unless a member starts there: then the
                    // body was never closed, which its owner reports.
                    if !self.at_body_close() && !self.at_member_start() {
                        let token = self.current();
                        self.push_error(
                            token.span,
                            &format!(
                                "expected the end of the body after a return (§7.2), found {}",
                                self.found(token)
                            ),
                        );
                        self.skip_rest_of_body();
                    }
                    break;
                }
                TokenKind::Dot => {
                    let dot = self.bump();
                    self.push_error(dot.span, "a `.` ends a statement, and none is here");
                }
                _ => {
                    let before = self.position;
                    let Some(expression) = self.parse_statement() else {
                        break;
                    };
                    statements.push(Statement::Expression(expression));
                    if self.eat(TokenKind::Dot) || self.at_body_close() {
                        continue;
                    }
                    if self.at_member_start() {
                        break;
                    }
                    // A statement the expression did not end: skip to the next
                    // one, so one error does not end the body. An expression
                    // that consumed nothing has reported its token already.
                    if self.position != before {
                        let token = self.current();
                        self.push_error(
                            token.span,
                            &format!(
                                "expected `.` between statements, found {}",
                                self.found(token)
                            ),
                        );
                    }
                    self.skip_rest_of_statement();
                }
            }
        }
        let end = if self.at_body_close() && !self.at(TokenKind::EndOfInput) {
            self.current().span
        } else {
            self.previous_span()
        };
        CodeBody {
            temporaries,
            temporaries_delimiters,
            statements,
            span: start.merge(end),
        }
    }

    /// A statement's expression. When it reported an error inside a member
    /// that starts in what it consumed, the body was never closed and the
    /// member is the next one in the class: the expression is parsed again
    /// up to that member start, its first diagnostics dropped, so the member
    /// is read as one and the half-typed method keeps the class around it. A
    /// statement without an error is never parsed again, so a valid parse
    /// stays as it is. Answers `None` when the member starts at the
    /// statement's first token: there is no statement.
    fn parse_statement(&mut self) -> Option<Expression> {
        let checkpoint = self.checkpoint();
        let expression = self.parse_expression();
        let first_error = self.diagnostics[checkpoint.1..]
            .iter()
            .find(|diagnostic| diagnostic.severity == crate::diagnostic::Severity::Error)
            .map(|diagnostic| diagnostic.span.start);
        let Some(first_error) = first_error else {
            return Some(expression);
        };
        let reached = self.position;
        let reached_offset = self.current().span.start;
        let member_start = self.member_holding_error(checkpoint.0, first_error);
        // Found by its offset: a negative literal the speculative parses
        // split in two moves the index on.
        self.position = (reached..self.tokens.len())
            .find(|index| self.tokens[*index].span.start == reached_offset)
            .unwrap_or(reached);
        let Some(member_start) = member_start else {
            return Some(expression);
        };
        self.restore(checkpoint);
        if member_start == checkpoint.0 {
            return None;
        }
        // The member start reads as the end of input while the expression is
        // parsed again: `bump` never passes one, so nothing reaches past it.
        // Set and lifted around one call, whatever it parses -- a block's
        // body bounds its own statements the same way, inside this bound.
        let bound = self.tokens[member_start];
        self.tokens[member_start].kind = TokenKind::EndOfInput;
        self.bounds.push(bound.span);
        let expression = self.parse_expression();
        self.bounds.pop();
        // A negative literal split in two before the bound moves it on.
        let index = (member_start..self.tokens.len()).find(|index| {
            self.tokens[*index].span == bound.span
                && self.tokens[*index].kind == TokenKind::EndOfInput
        });
        debug_assert!(index.is_some(), "the bound left the token stream");
        if let Some(index) = index {
            self.tokens[index] = bound;
        }
        Some(expression)
    }

    /// The first position, from `first` up to the statement's first error,
    /// at which a member starts that holds that error: one that parses
    /// without an error of its own through its `)`, or, its body clean, up to
    /// where an unclosed body ends. An ordinary comparison such as `size =
    /// (limit + 1)` reads as a member too, so one that ends before the error,
    /// or fails itself, is no member the statement swallowed.
    fn member_holding_error(&mut self, first: usize, first_error: u32) -> Option<usize> {
        let mut position = first;
        while self.tokens.get(position).is_some_and(|token| {
            token.span.start <= first_error && token.kind != TokenKind::EndOfInput
        }) {
            if self.member_starts_at(position)
                && self
                    .speculative_member_end(position)
                    .is_some_and(|end| first_error < end)
            {
                return Some(position);
            }
            position += 1;
        }
        None
    }

    /// Where a member parsed from `position` ends, if it parses cleanly: see
    /// `member_holding_error`. Nothing is kept, and the cursor stays.
    fn speculative_member_end(&mut self, position: usize) -> Option<u32> {
        let cursor = self.position;
        self.position = position;
        let checkpoint = self.checkpoint();
        // Each speculative member can hold another: the depth guard bounds
        // the chain a run of half-typed members makes.
        let end = if self.enter() {
            self.clean_member_end()
        } else {
            None
        };
        self.leave();
        self.restore(checkpoint);
        self.position = cursor;
        end
    }

    /// `speculative_member_end`'s parse, at the member start.
    fn clean_member_end(&mut self) -> Option<u32> {
        let diagnostics = self.diagnostics.len();
        if self.at_nested_class() {
            let _ = self.parse_class_declaration();
            return (self.diagnostics.len() == diagnostics).then(|| self.previous_span().end);
        }
        if self.at_lazy_slot() {
            let _ = self.parse_lazy_slot_declaration();
            self.eat(TokenKind::Dot);
            return Some(self.previous_span().end);
        }
        if !self.looks_like_method() {
            return None;
        }
        let _ = self.parse_access_modifier();
        let _ = self.parse_message_pattern();
        self.expect(TokenKind::Equals);
        self.expect(TokenKind::LeftParenthesis);
        let _ = self.parse_code_body();
        if self.diagnostics.len() != diagnostics {
            return None;
        }
        if self.at(TokenKind::RightParenthesis) {
            return Some(self.bump().span.end);
        }
        (self.at(TokenKind::EndOfInput) || self.at_member_start()).then(|| self.previous_span().end)
    }

    /// `at_member_start` at another position, the cursor left where it was.
    fn member_starts_at(&mut self, position: usize) -> bool {
        let cursor = self.position;
        self.position = position;
        let starts = self.at_member_start();
        self.position = cursor;
        starts
    }

    /// After an error in a statement: skip to the `.` that ends it and past
    /// it. A stray `}` that closes nothing is skipped as well, so the body
    /// always moves on.
    fn skip_rest_of_statement(&mut self) {
        if !self.skip_balanced(&[TokenKind::Dot]) && self.at(TokenKind::RightBrace) {
            self.bump();
        }
        self.eat(TokenKind::Dot);
    }

    /// After a statement nothing may follow: skip to the body's close, or to
    /// a member start, stray `}` included.
    fn skip_rest_of_body(&mut self) {
        loop {
            self.skip_balanced(&[]);
            if !self.at(TokenKind::RightBrace) {
                break;
            }
            self.bump();
        }
    }

    /// `codeBody` temporaries. Per the grammar
    /// (`corpus/reference/NewspeakGrammar.ns:148-150`), `temporaries =
    /// slotDecls`, i.e. a temporary list has the full slot-declaration form,
    /// including initialized locals `| x = expression. |`. `CodeBody::temporaries`
    /// keeps the full declaration, so the runtime can initialize each slot in
    /// declaration order (§3.6) and tell an immutable one from a mutable one;
    /// a temporary's `lazy` is still dropped, since the grammar gives a
    /// temporary no `lazy` form -- a class body's is kept, `lazy: true`, in
    /// `parse_lazy_slot_declaration`. A code body never
    /// legitimately starts with a binary `|` send (there is no left operand),
    /// so a leading `Bar` / `DoubleBar` is unambiguously a temporary section.
    fn parse_temporaries(&mut self) -> (Vec<SlotDeclaration>, Option<Delimiters>) {
        if !self.at(TokenKind::Bar) && !self.at(TokenKind::DoubleBar) {
            return (Vec::new(), None);
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
                | TokenKind::DoubleBar
        ) && super::is_name(self.peek2())
    }

    pub(crate) fn parse_message_pattern(&mut self) -> MessagePattern {
        // Declarations may carry `<Type>` annotations after each parameter name,
        // and a `^<Type>` return annotation before the `=`; Milestone 1 discards
        // them via `skip_type_annotation` / `skip_return_type_annotation`.
        let pattern = if self.at_binary_selector_pattern() {
            let selector = self.identifier_from_bump();
            let parameter = self.declared_identifier("name a parameter");
            self.skip_type_annotation();
            MessagePattern::Binary {
                selector,
                parameter,
            }
        } else {
            match self.peek() {
                // `keywordMsgPattern → (keyword slotDecl)+` (§6.3.4): a setter
                // keyword `name::` is not a keyword.
                TokenKind::Keyword => {
                    let mut parts = Vec::new();
                    while self.at(TokenKind::Keyword) {
                        let keyword = self.bump();
                        let key = Identifier {
                            name: self.text(keyword).trim_end_matches(':').to_string(),
                            span: keyword.span,
                        };
                        let parameter = self.declared_identifier("name a parameter");
                        self.skip_type_annotation();
                        parts.push((key, parameter));
                    }
                    MessagePattern::Keyword { parts }
                }
                _ => {
                    let selector = self.declared_identifier("be a message selector");
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

    /// §6's `typeParameterDecl → langleBracket lbracket identifier (comma
    /// identifier)* rbracket rangleBracket`, the cursor at its `<`: the names
    /// are checked and dropped, since there is no type AST.
    fn parse_type_parameter_declaration(&mut self) {
        self.bump(); // `<`
        self.bump(); // `[`
        loop {
            let _ = self.declared_identifier("name a type parameter");
            if !(self.at(TokenKind::BinarySelector) && self.text(self.current()) == ",") {
                break;
            }
            self.bump(); // `,`
        }
        self.expect(TokenKind::RightBracket);
        self.expect(TokenKind::GreaterThan);
    }

    /// Whether the cursor is at a binary factory pattern whose selector is
    /// `=`, followed by the header's own `=`.
    fn at_binary_equals_factory(&mut self) -> bool {
        if !self.at_binary_selector_pattern() {
            return false;
        }
        let checkpoint = self.checkpoint();
        let _ = self.parse_message_pattern();
        let matched = self.at(TokenKind::Equals);
        self.restore(checkpoint);
        matched
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
        let open = self.expect(TokenKind::LeftParenthesis);
        let body = self.parse_code_body();
        let close = self.close_method_body();
        let end = close.map_or_else(|| self.previous_span(), |token| token.span);
        MethodDeclaration {
            access,
            pattern,
            body,
            body_delimiters: open.map(|open| Delimiters {
                open: open.span,
                close: close.map(|token| token.span),
            }),
            span: start.merge(end),
        }
    }

    /// A method body's `)`. When it is missing and a member starts here, the
    /// method ends where its body did, so the member after it is read as
    /// one -- the method being typed keeps the class around it. Otherwise
    /// what is left is skipped, balanced, up to the method's `)`.
    fn close_method_body(&mut self) -> Option<Token> {
        if self.at(TokenKind::RightParenthesis) {
            return Some(self.bump());
        }
        let token = self.current();
        self.push_error(
            token.span,
            &format!(
                "expected `)` to end the method body, found {}",
                self.found(token)
            ),
        );
        self.skip_rest_of_body();
        self.eat_token(TokenKind::RightParenthesis)
    }

    /// `slotDecls → seqSlotDecls | simSlotDecls` (§6.3.2), `| slotDef* |` or
    /// `|| slotDef* ||`. The lexer emits `||` as one token, and `||` is also
    /// an empty sequential section, `| |` without its space. The canonical
    /// grammar tries `simSlotDecls` first and falls back to the sequential
    /// reading when it does not match -- `[:x | || x + 1]` is an empty
    /// section and a statement -- and so does this. (The specification lists
    /// `seqSlotDecls` first, which would read every `||` as the empty
    /// section and leave `simSlotDecls` unreachable.)
    pub(crate) fn parse_slot_declarations(&mut self) -> (Vec<SlotDeclaration>, Option<Delimiters>) {
        if self.at(TokenKind::DoubleBar) {
            let checkpoint = self.checkpoint();
            let section = self.parse_slot_section(TokenKind::DoubleBar);
            if self.diagnostics.len() == checkpoint.1 {
                return section;
            }
            self.restore(checkpoint);
            let empty = self.bump(); // the empty sequential section
            return (
                Vec::new(),
                Some(Delimiters {
                    open: empty.span,
                    close: Some(empty.span),
                }),
            );
        }
        self.parse_slot_section(TokenKind::Bar)
    }

    /// One section, between two `close` tokens.
    fn parse_slot_section(
        &mut self,
        close: TokenKind,
    ) -> (Vec<SlotDeclaration>, Option<Delimiters>) {
        let Some(open) = self.eat_token(close) else {
            return (Vec::new(), None);
        };
        let mut slots = Vec::new();
        while !self.at_slot_section_end(close) {
            let start = self.current().span;
            // `slotDef → [accessModifier] slotDecl ...` (§6.3.2), a PEG: an
            // access modifier's word is taken as the modifier whenever it
            // stands first, and never tried again as the name, so
            // `| public |` declares nothing and is refused, while
            // `| public public |` declares a public slot named `public`.
            let access = self.parse_access_modifier();
            if !self.at_name() {
                let token = self.current();
                self.push_error(token.span, "expected a slot name");
                // The section's close stays for `expect` below, and an
                // owner's bracket for its owner; a token the skip refuses
                // that is neither is taken, so the loop always moves on.
                let before = self.position;
                self.skip_balanced(&[TokenKind::Dot, close]);
                if self.position == before && !self.at_slot_section_end(close) {
                    self.bump();
                }
                self.eat(TokenKind::Dot);
                continue;
            }
            let name = self.declared_identifier("name a slot or a temporary");
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
            // §6.3.2: "The trailing dot is mandatory after an initializer
            // and optional after an uninitialized slot".
            if !self.eat(TokenKind::Dot) && initializer.is_some() {
                self.push_error(
                    self.current().span,
                    "expected `.` after a slot's initializer (§6.3.2)",
                );
            }
            slots.push(SlotDeclaration {
                access,
                name,
                mutability,
                initializer,
                lazy: false,
                simultaneous: close == TokenKind::DoubleBar,
                span: start.merge(end),
            });
        }
        let closing = self.expect(close);
        (
            slots,
            Some(Delimiters {
                open: open.span,
                close: closing.map(|token| token.span),
            }),
        )
    }

    /// What ends a slot section's loop: its closing bar, or a closing
    /// bracket, which no slot declaration holds at its top level -- the
    /// section was never closed, and the bracket is its owner's.
    fn at_slot_section_end(&self, close: TokenKind) -> bool {
        self.at(close)
            || matches!(
                self.peek(),
                TokenKind::RightParenthesis
                    | TokenKind::RightBracket
                    | TokenKind::RightBrace
                    | TokenKind::EndOfInput
            )
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
    /// Top level: `languageId? category? classDecl EndOfInput`.
    ///
    /// `languageId` is a leading name (`Newspeak3`); `category` is a leading
    /// `String` literal. Missing pieces and trailing tokens are reported but
    /// parsing continues (recovery).
    pub(crate) fn parse_module(&mut self) -> Module {
        let start = self.current().span;
        // §8.1 requires the language id; ns-next makes it optional, because
        // nothing reads it (`docs/design/frontend.md`, "The module header").
        // An identifier can only be the id, but `class` and the access
        // modifiers are unreserved words that also open a declaration: one
        // of them is the id only when a `toplevelClass` follows it.
        let is_language_id = match self.peek() {
            TokenKind::Identifier => true,
            kind if super::is_name(kind) => self.toplevel_class_starts_at(self.position + 1),
            _ => false,
        };
        let language_id = if is_language_id {
            let token = self.bump();
            self.text(token).to_string()
        } else {
            String::new()
        };
        // §8.1's `toplevelClass → classCategory classDeclaration`, with
        // `classCategory → [string]`: the category is optional, and a module's
        // class, which nothing encloses, takes no access modifier.
        let category = if self.at(TokenKind::String) {
            let token = self.bump();
            string_lexeme_body(self.text(token))
        } else {
            String::new()
        };
        if matches!(
            self.peek(),
            TokenKind::Public | TokenKind::Protected | TokenKind::Private
        ) {
            self.push_error(
                self.current().span,
                "a module's class takes no access modifier; only a nested class does (§8.1)",
            );
        }
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

    /// Whether the tokens from `position` on open a `toplevelClass`: a
    /// category, or `class` and the class's name, an access modifier before
    /// it included (refused afterwards, but a declaration all the same).
    fn toplevel_class_starts_at(&self, position: usize) -> bool {
        let kind_at = |offset: usize| {
            self.tokens
                .get(position + offset)
                .map_or(TokenKind::EndOfInput, |token| token.kind)
        };
        let class_offset = match kind_at(0) {
            TokenKind::String => return true,
            TokenKind::Public | TokenKind::Protected | TokenKind::Private => 1,
            _ => 0,
        };
        kind_at(class_offset) == TokenKind::Class && super::is_name(kind_at(class_offset + 1))
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
    /// So a class is
    /// `class Id pattern? = superclause? ( '<:' clause )* ( G1 ) ( G2 ) ( ':' '(' G3 ')' )?`
    /// with three distinct paren groups (the `<:` clauses are §6.2's mixin
    /// application; with at least one of them the whole body may be replaced by
    /// a single `.`):
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
        let name = self.declared_identifier("name a class");
        // A generic class's type parameters, dropped: §6's
        // `typeParameterDecl`, `class Map <[K, V]> ...`, and the canonical
        // grammar's `class List[E] ...` (`NewspeakGrammar.ns:465`).
        if self.at(TokenKind::LessThan) && self.peek2() == TokenKind::LeftBracket {
            self.parse_type_parameter_declaration();
        }
        self.skip_type_arguments();
        // §6's two alternatives, `messagePattern equalSign ...` before
        // `empty equalSign ...`: an `=` here is the factory's binary selector
        // when a parameter and another `=` follow it (`class Pair = other =
        // Object new () ()`), and otherwise the one that ends the header.
        let factory = if self.at(TokenKind::Equals) && !self.at_binary_equals_factory() {
            None
        } else {
            Some(self.parse_message_pattern())
        };
        self.expect(TokenKind::Equals);
        let superclass = if self.at(TokenKind::LeftParenthesis) {
            None
        } else {
            Some(self.parse_superclass_clause())
        };
        // §6.2's mixin application, `class C = S someMessage <: M otherMessage`:
        //
        // ```text
        // explicitInheritanceListAndOrBody = inheritanceClause mixinSpec
        // mixinSpec       = classBody | mixinAppSuffix
        // mixinAppSuffix  = (mixinOperator inheritanceClause)+ (dot | classBody)
        // ```
        //
        // Each `<:` clause is §6.1's `inheritanceClause`, the same production
        // the superclass clause above is, so `parse_superclass_clause` parses
        // it unchanged rather than a near-copy drifting from it. The suffix may
        // only follow an explicit superclass clause, which is why it is parsed
        // here and not where an implicit `Object` superclass is assumed.
        let mut mixin_applications = Vec::new();
        while self.eat(TokenKind::MixinOperator) {
            mixin_applications.push(self.parse_superclass_clause());
        }
        // `(dot | classBody)`: a mixin application may end at a `.` instead of
        // carrying a body. It then declares no slots, no methods, no nested
        // classes, no class side and no class comment — the applied mixin
        // supplies all of its behaviour.
        if !mixin_applications.is_empty() && self.at(TokenKind::Dot) {
            let dot = self.bump();
            let span = start.merge(dot.span);
            return ClassDeclaration {
                access,
                name,
                factory,
                superclass,
                mixin_applications,
                class_comment: None,
                instance_side: ClassBody {
                    slots: Vec::new(),
                    initializer_expressions: Vec::new(),
                    methods: Vec::new(),
                    nested_classes: Vec::new(),
                    delimiters: None,
                    slot_delimiters: None,
                    span,
                },
                class_side: None,
                span,
            };
        }

        // --- G1: classHeader body `( classComment? slotDecls? initExprs )` ---
        let instance_open = self.expect(TokenKind::LeftParenthesis);
        let class_comment = self.take_class_comment();
        let (header_slots, slot_delimiters, initializer_expressions, g1_span) =
            self.parse_header_body();
        let g1_end = self.end_of_group();
        self.expect(TokenKind::RightParenthesis);

        // --- G2: sideDecl body `( ( classDecl | lazySlotDecl | methodDecl )* )` ---
        // §6.3's `classBody → classHeader sideDecl [classSideDecl]`: the
        // instance side is not optional, however empty.
        let (lazy_slots, methods, nested_classes, g2_end, instance_close) =
            if self.at(TokenKind::LeftParenthesis) {
                self.bump(); // `(`
                let (lazy, methods, nested) = self.parse_side_body();
                let end = self.end_of_group();
                let close = self.expect(TokenKind::RightParenthesis);
                (lazy, methods, nested, end, close)
            } else {
                self.push_error(
                    self.current().span,
                    "expected the instance side's `( ... )` after the class header (§6.3)",
                );
                (Vec::new(), Vec::new(), Vec::new(), g1_end, None)
            };

        let mut slots = header_slots;
        slots.extend(lazy_slots);
        let instance_side = ClassBody {
            slots,
            initializer_expressions,
            methods,
            nested_classes,
            delimiters: instance_open.map(|open| Delimiters {
                open: open.span,
                close: instance_close.map(|token| token.span),
            }),
            slot_delimiters,
            span: g1_span.merge(g2_end),
        };

        // --- G3: classSideDecl body `: ( methodDecl* )` ---
        let class_side = if self.eat(TokenKind::Colon) {
            let open = self.expect(TokenKind::LeftParenthesis);
            // A class-side comment is allowed but not retained.
            let _ = self.take_class_comment();
            let mut body = self.parse_class_side_body();
            let close = self.expect(TokenKind::RightParenthesis);
            body.delimiters = open.map(|open| Delimiters {
                open: open.span,
                close: close.map(|token| token.span),
            });
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
            mixin_applications,
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

    /// `[inheritancePrefix] unarySelector [message]` (§6.1) — the receiver's
    /// name and at most one message to it (`Object new`, `superclass
    /// subclassMixin: m`).
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
                    Some(SuperclassPrefix::Outer(
                        self.declared_identifier("name an enclosing class"),
                    ))
                }
                _ => None,
            },
            _ => None,
        };
        // §6.1's `inheritanceClause → [inheritancePrefix] unarySelector
        // [message]`: the receiver is a unary selector, an identifier, never
        // a literal or a reserved word.
        let receiver = self.declared_identifier("name a superclass");
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

    /// The clause's `[message]` (§6.1's `inheritanceClause → [inheritancePrefix]
    /// unarySelector [message]`, `message → keywordMsg | unarySelector |
    /// binaryMsg`): at most one message to the class the receiver names, whose
    /// arguments may be any binary or unary expressions.
    fn parse_trailing_sends(&mut self, out: &mut Vec<MessageSend>) {
        if self.at_name() && !self.is_reserved_identifier(self.current()) {
            let selector_token = self.bump();
            out.push(MessageSend {
                selector: Selector::Unary(self.text(selector_token).to_string()),
                arguments: Vec::new(),
                span: selector_token.span,
            });
        } else if self.at_binary_operator() {
            if self.at(TokenKind::Number) {
                self.split_negative_number();
            }
            let operator = self.bump();
            let argument = self.parse_unary_message();
            out.push(MessageSend {
                selector: Selector::Binary(self.text(operator).to_string()),
                span: operator
                    .span
                    .merge(super::expression::expression_span(&argument)),
                arguments: vec![argument],
            });
        } else if self.at(TokenKind::Keyword) {
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
        if self.at_name() || self.at_binary_operator() || self.at(TokenKind::Keyword) {
            self.push_error(
                self.current().span,
                "a superclass clause sends at most one message to the class it names (§6.1)",
            );
        }
    }

    /// **G1** — `classHeader` body (`NewspeakGrammar.ns:208`):
    /// `classComment? , slotDecls? , initExprs`. The class comment is consumed
    /// by the caller via [`Parser::take_class_comment`] right after the `(`.
    /// Here: an optional single slot section (`| .. |` or `|| .. ||`), then
    /// zero or more `Dot`-separated/terminated initializer expressions, until `)` or
    /// end of input. Methods, nested classes and a second slot section are NOT
    /// accepted here — they belong to G2.
    fn parse_header_body(
        &mut self,
    ) -> (
        Vec<SlotDeclaration>,
        Option<Delimiters>,
        Vec<Statement>,
        crate::span::Span,
    ) {
        let start = self.current().span;
        let (slots, slot_delimiters) = if self.at(TokenKind::Bar) || self.at(TokenKind::DoubleBar) {
            self.parse_slot_declarations()
        } else {
            (Vec::new(), None)
        };
        let mut initializer_expressions = Vec::new();
        loop {
            match self.peek() {
                TokenKind::RightParenthesis | TokenKind::EndOfInput => break,
                // `initExprs → [expression (dot expression)* [dot]]`.
                TokenKind::Dot => {
                    let dot = self.bump();
                    self.push_error(
                        dot.span,
                        "a `.` ends an initializer expression, and none is here",
                    );
                }
                _ => {
                    let before = self.position;
                    let expression = self.parse_expression();
                    if !self.eat(TokenKind::Dot)
                        && !matches!(
                            self.peek(),
                            TokenKind::RightParenthesis | TokenKind::EndOfInput
                        )
                    {
                        self.push_error(
                            self.current().span,
                            "expected `.` between initializer expressions (§6.3)",
                        );
                    }
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
        (
            slots,
            slot_delimiters,
            initializer_expressions,
            start.merge(end),
        )
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
                // `sideDecl → lparen (nestedClassDecl | lazySlotDecl |
                // methodDecl)* rparen` has no separator.
                TokenKind::Dot => {
                    let dot = self.bump();
                    self.push_error(dot.span, "no `.` stands between a class's members (§6.3)");
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
                // `sideDecl`'s alternatives in their PEG order: a nested class
                // above, then a lazy slot, then a method. Each is tried in
                // full before the next, so `private lazy = ( ^1 )`, which is
                // no lazy slot, is a method named `lazy`.
                _ if self.at_lazy_slot() => {
                    lazy_slots.push(self.parse_lazy_slot_declaration());
                }
                _ if self.looks_like_method() => {
                    methods.push(self.parse_method_declaration());
                }
                // Neither matched in full: a declaration that starts as a lazy
                // slot is reported as one, so its error names what is missing.
                _ if self.at_lazy_slot_prefix() => {
                    lazy_slots.push(self.parse_lazy_slot_declaration());
                }
                _ => {
                    let token = self.current();
                    self.push_error(
                        token.span,
                        "expected a method, nested class, or lazy slot declaration",
                    );
                    // One error for the whole run, up to the next member.
                    if !self.skip_balanced(&[]) && !self.at(TokenKind::RightParenthesis) {
                        self.bump(); // a stray `]` or `}`
                    }
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
                    let dot = self.bump();
                    self.push_error(
                        dot.span,
                        "no `.` stands between a class side's methods (§6.3)",
                    );
                }
                _ if self.looks_like_method() => {
                    methods.push(self.parse_method_declaration());
                }
                _ => {
                    let token = self.current();
                    self.push_error(token.span, "expected a method declaration");
                    // One error for the whole run, up to the next method.
                    if !self.skip_balanced(&[]) && !self.at(TokenKind::RightParenthesis) {
                        self.bump(); // a stray `]` or `}`
                    }
                }
            }
            if self.position == before {
                let token = self.bump();
                self.push_error(token.span, "unexpected token on the class side");
            }
        }
        let end = self.end_of_group();
        ClassBody {
            slots: Vec::new(),
            initializer_expressions: Vec::new(),
            methods,
            nested_classes: Vec::new(),
            delimiters: None,
            slot_delimiters: None,
            span: start.merge(end),
        }
    }

    /// Where a class's `( … )` group ends: its `)`, or, when recovery found
    /// none, the last token it consumed.
    fn end_of_group(&self) -> crate::span::Span {
        if self.at(TokenKind::RightParenthesis) {
            self.current().span
        } else {
            self.previous_span()
        }
    }

    /// A nested class is `accessModifier? 'class' classId ...` — the token
    /// after `class` must be the class name, any name (`class public = ...`
    /// declares a class named `public`). `public class = (...)` (a method
    /// literally named `class`, common in the mirror libraries) has `=`
    /// there, so it is NOT a nested class. Once `class` and a name are seen
    /// neither of the other two alternatives can match, so the parser commits
    /// to the class and reports its errors rather than retrying.
    fn at_nested_class(&self) -> bool {
        let class_position = if matches!(
            self.peek(),
            TokenKind::Public | TokenKind::Protected | TokenKind::Private
        ) {
            self.position + 1
        } else {
            self.position
        };
        let kind_at = |position: usize| self.tokens.get(position).map(|token| token.kind);
        kind_at(class_position) == Some(TokenKind::Class)
            && kind_at(class_position + 1).is_some_and(super::is_name)
    }

    fn at_lazy_word(&self, position: usize) -> bool {
        self.tokens
            .get(position)
            .is_some_and(|token| token.kind == TokenKind::Identifier && self.text(*token) == "lazy")
    }

    /// §6.3.3's `lazySlotDecl`, a PEG:
    ///
    /// ```text
    /// ((lazyModifier [accessModifier]) | (accessModifier lazyModifier))
    ///     slotDecl (equalSign | "::=") expression dot
    /// ```
    ///
    /// `lazy` lexes as an `Identifier`; it is the modifier only here. Whether
    /// the whole production matches decides it, not its first tokens: `lazy =
    /// ( ^1 )` and `private lazy = ( ^1 )` are no lazy slot but methods named
    /// `lazy`, and `lazy lazy = 1.` is a lazy slot named `lazy`. The optional
    /// access modifier is taken whenever it stands there and never retried
    /// as the name, so `lazy public = 1.` matches nothing.
    fn at_lazy_slot(&mut self) -> bool {
        let checkpoint = self.checkpoint();
        let matched = self.lazy_slot_modifiers().is_some()
            && self.at_name()
            && {
                self.bump(); // the name
                self.skip_type_annotation();
                self.eat(TokenKind::Equals) || self.eat(TokenKind::ColonColonEquals)
            }
            && {
                let diagnostics = self.diagnostics.len();
                let _ = self.parse_expression();
                self.diagnostics.len() == diagnostics
            }
            && self.at(TokenKind::Dot);
        self.restore(checkpoint);
        matched
    }

    /// Whether the cursor sits at a lazy slot's modifiers and a name, which
    /// is how a declaration that means to be one starts. It only decides how
    /// a declaration no alternative matches is reported.
    fn at_lazy_slot_prefix(&mut self) -> bool {
        let checkpoint = self.checkpoint();
        let matched = self.lazy_slot_modifiers().is_some() && self.at_name();
        self.restore(checkpoint);
        matched
    }

    /// Consumes `lazy [accessModifier]` or `accessModifier lazy` and answers
    /// the access modifier; the outer `None` means neither is there.
    fn lazy_slot_modifiers(&mut self) -> Option<Option<AccessModifier>> {
        if self.at_lazy_word(self.position) {
            self.bump(); // `lazy`
            return Some(self.parse_access_modifier());
        }
        if matches!(
            self.peek(),
            TokenKind::Public | TokenKind::Protected | TokenKind::Private
        ) && self.at_lazy_word(self.position + 1)
        {
            let access = self.parse_access_modifier();
            self.bump(); // `lazy`
            return Some(access);
        }
        None
    }

    /// Parse a lazy slot declaration (§6.3.3), in either order of `lazy` and
    /// the access modifier. The caller has checked that the declaration
    /// starts as one (`at_lazy_slot_prefix`).
    fn parse_lazy_slot_declaration(&mut self) -> SlotDeclaration {
        let start = self.current().span;
        let access = self.lazy_slot_modifiers().flatten();
        let name = self.declared_identifier("name a slot or a temporary");
        self.skip_type_annotation();
        let mutability = if self.eat(TokenKind::ColonColonEquals) {
            SlotMutability::MutableInitialized
        } else {
            self.expect(TokenKind::Equals);
            SlotMutability::Immutable
        };
        let initializer = Some(self.parse_expression());
        let end = self.current().span;
        if !self.eat(TokenKind::Dot) {
            self.push_error(
                self.current().span,
                "expected `.` after a lazy slot's initializer (§6.3.3)",
            );
        }
        SlotDeclaration {
            access,
            name,
            mutability,
            initializer,
            lazy: true,
            simultaneous: false,
            span: start.merge(end),
        }
    }

    /// Whether a member of a class side starts at the cursor -- a nested
    /// class, a lazy slot or a method, an access modifier before it
    /// included. A recovery skip stops here, so the member after an error is
    /// read as one rather than swallowed.
    pub(super) fn at_member_start(&mut self) -> bool {
        self.at_nested_class() || self.at_lazy_slot_prefix() || self.looks_like_method()
    }

    /// Skips what is left of a construct after an error, each bracketed
    /// group whole, so a recovery never stops at a bracket inside a group nor
    /// takes a bracket that closes an enclosing one. Stops -- consuming
    /// nothing more -- at a token of `stops` outside every group, at a
    /// closing bracket no skipped group opened, at a member start, or at the
    /// end of input. Answers whether it skipped anything.
    pub(super) fn skip_balanced(&mut self, stops: &[TokenKind]) -> bool {
        let start = self.position;
        let mut expected_closers: Vec<TokenKind> = Vec::new();
        loop {
            let kind = self.peek();
            if kind == TokenKind::EndOfInput || self.at_member_start() {
                break;
            }
            match kind {
                TokenKind::LeftParenthesis => expected_closers.push(TokenKind::RightParenthesis),
                TokenKind::LeftBracket => expected_closers.push(TokenKind::RightBracket),
                TokenKind::LeftBrace => expected_closers.push(TokenKind::RightBrace),
                TokenKind::RightParenthesis | TokenKind::RightBracket | TokenKind::RightBrace => {
                    match expected_closers
                        .iter()
                        .rposition(|expected| *expected == kind)
                    {
                        Some(index) => expected_closers.truncate(index),
                        None => break,
                    }
                }
                _ if expected_closers.is_empty() && stops.contains(&kind) => break,
                _ => {}
            }
            self.bump();
        }
        self.position != start
    }

    /// Whether the cursor is at what ends a code body: its `)` or `]`, or
    /// the end of input.
    pub(super) fn at_body_close(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::RightParenthesis | TokenKind::RightBracket | TokenKind::EndOfInput
        )
    }

    /// Speculative lookahead: does the cursor sit at `accessModifier?
    /// messagePattern '=' '('` (a method), rather than an initializer expression?
    fn looks_like_method(&mut self) -> bool {
        let checkpoint = self.checkpoint();
        self.parse_access_modifier();
        // A bare `= (` starts no method: without a pattern there is none.
        let is_method = self.try_parse_message_pattern().is_some()
            && self.at(TokenKind::Equals)
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
                TokenKind::Keyword => {
                    while self.at(TokenKind::Keyword) {
                        self.bump();
                        if self.at_name() {
                            self.bump();
                        } else {
                            return None;
                        }
                        self.skip_type_annotation();
                    }
                    Some(())
                }
                kind if super::is_name(kind) => {
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
        self.parse_slot_declarations().0
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
    use crate::span::Span;
    use indoc::indoc;

    fn module(source: &str) -> Module {
        match crate::parse(source) {
            Ok(module) => module,
            Err(diagnostics) => panic!("parse errors: {diagnostics:#?}"),
        }
    }

    /// The editor folds from an opening token to its closing one, so the
    /// AST records both for every construct that folds.
    #[test]
    fn every_folding_construct_records_its_brackets() {
        let source = indoc! {"
            class Module = (
                | first second |
            ) (
                run = (
                    | temporary |
                    ^[:each | each]
                )
            ) : (
                make = ( ^self new )
            )
        "};
        let module = module(source);
        let text_of = |delimiters: Option<Delimiters>| {
            let delimiters = delimiters.expect("the brackets are recorded");
            let close = delimiters.close.expect("the closing token is there");
            Span::new(delimiters.open.start, close.end)
                .text(source)
                .to_string()
        };
        let root = &module.root_class;
        let instance = text_of(root.instance_side.delimiters);
        assert!(
            instance.starts_with("(\n    | first second |\n) ("),
            "{instance}"
        );
        assert!(
            instance.ends_with("^[:each | each]\n    )\n)"),
            "{instance}"
        );
        assert_eq!(
            text_of(root.instance_side.slot_delimiters),
            "| first second |"
        );
        let class_side = root.class_side.as_ref().expect("a class side");
        assert_eq!(
            text_of(class_side.delimiters),
            "(\n    make = ( ^self new )\n)"
        );
        let run = &root.instance_side.methods[0];
        let body = text_of(run.body_delimiters);
        assert!(body.starts_with("(\n        | temporary |"), "{body}");
        assert!(body.ends_with("each]\n    )"), "{body}");
        assert_eq!(text_of(run.body.temporaries_delimiters), "| temporary |");
        let Statement::Return(Expression::Block(block)) = &run.body.statements[0] else {
            panic!("a returned block");
        };
        assert_eq!(text_of(block.delimiters), "[:each | each]");
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
            class C = Object new () ()
        "});
        let superclass = module.root_class.superclass.as_ref().unwrap();
        assert_eq!(superclass.receiver.name, "Object");
        assert_eq!(superclass.messages.len(), 1);
    }

    #[test]
    fn a_mixin_application_carries_its_clauses_in_order() {
        let module = module(indoc! {"
            class Applied using: platform = Object new <: Value <: Other with: 1 (
            ) (
            ) : ()
        "});
        let declaration = &module.root_class;
        let superclass = declaration
            .superclass
            .as_ref()
            .expect("a superclass clause");
        assert_eq!(superclass.receiver.name, "Object");
        let applications = &declaration.mixin_applications;
        assert_eq!(applications.len(), 2);
        assert_eq!(applications[0].receiver.name, "Value");
        assert!(applications[0].messages.is_empty());
        assert_eq!(applications[1].receiver.name, "Other");
        assert_eq!(applications[1].messages.len(), 1);
    }

    #[test]
    fn a_mixin_application_may_end_with_a_dot_instead_of_a_body() {
        let module = module(indoc! {"
            class Applied = Object <: Value.
        "});
        let declaration = &module.root_class;
        assert_eq!(declaration.mixin_applications.len(), 1);
        assert!(declaration.instance_side.methods.is_empty());
        assert!(declaration.instance_side.slots.is_empty());
    }

    // A declaration without `<:` carries no mixin application at all, and the
    // `<` of a type annotation on the primary factory is still an annotation.
    #[test]
    fn an_ordinary_declaration_has_no_mixin_applications() {
        let module = module(indoc! {"
            class Plain using: platform <Platform> = Object new () ()
        "});
        assert!(module.root_class.mixin_applications.is_empty());
    }

    #[test]
    fn slots_methods_and_nested_class() {
        // Comment + `|..|` slot section + initializer expressions live in G1; methods and
        // nested classes live in G2.
        let source = indoc! {"
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
            class C = ( ^ 1 ) ()
        "})
        .is_err());
    }

    #[test]
    fn malformed_body_semicolon_is_error() {
        assert!(crate::parse(indoc! {"
            class C = ( ; ) ()
        "})
        .is_err());
    }

    #[test]
    fn malformed_body_binary_selector_is_error() {
        assert!(crate::parse(indoc! {"
            class C = ( + ) ()
        "})
        .is_err());
    }

    // A `|..|` slot section inside G2 is malformed (it belongs to G1).
    #[test]
    fn slot_section_in_g2_is_error() {
        assert!(crate::parse(indoc! {"
            class C = () ( | x | ) : ()
        "})
        .is_err());
    }

    // --- corpus-shaped tests (real two-group grammar) ---

    #[test]
    fn two_group_with_class_side() {
        // Exact shape of corpus/modules/ADummyPush.ns.
        let source = indoc! {"
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
            class C = ( x:: 1. y:: 2. ) ()
        "});
        let body = &module.root_class.instance_side;
        assert_eq!(body.initializer_expressions.len(), 2);
        assert!(body.methods.is_empty());
    }

    #[test]
    fn lazy_slot_in_g2() {
        let module = module(indoc! {"
            class C = () ( lazy foo = bar baz. ) : ()
        "});
        let body = &module.root_class.instance_side;
        assert_eq!(body.slots.len(), 1);
        assert_eq!(body.slots[0].name.name, "foo");
        assert_eq!(body.slots[0].mutability, SlotMutability::Immutable);
        assert!(
            body.slots[0].lazy,
            "a class body's `lazy` slot is lazy (§6.3.3)"
        );
    }

    #[test]
    fn lazy_slot_modifier_first_in_g2() {
        let module = module(indoc! {"
            class C = () ( public lazy foo ::= bar. ) : ()
        "});
        let body = &module.root_class.instance_side;
        assert_eq!(body.slots.len(), 1);
        assert_eq!(body.slots[0].access, Some(AccessModifier::Public));
        assert_eq!(body.slots[0].mutability, SlotMutability::MutableInitialized);
        assert!(body.slots[0].lazy);
    }

    #[test]
    fn a_header_slot_is_never_lazy() {
        let module = module(indoc! {"
            class C = ( | foo = 1. | ) ( ) : ()
        "});
        let body = &module.root_class.instance_side;
        assert_eq!(body.slots.len(), 1);
        assert!(!body.slots[0].lazy);
    }

    // A method literally named `lazy` must not be mistaken for a lazy slot.
    #[test]
    fn method_named_lazy_is_not_a_slot() {
        let module = module(indoc! {"
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

    /// A chain of setter keywords recurses once per keyword; past the
    /// nesting limit it is a diagnostic, never a stack overflow.
    #[test]
    fn a_long_setter_chain_does_not_overflow() {
        let source = format!(
            "class Module = () ( run = ( {}1 ) ) : ()",
            "x:: ".repeat(100_000)
        );
        let Err(diagnostics) = crate::parse(&source) else {
            panic!("a chain past the limit is refused");
        };
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message == "expression nested too deeply"));
    }

    /// Nested pattern literals recurse once per level, past the limit a
    /// diagnostic as well.
    #[test]
    fn deeply_nested_pattern_literals_do_not_overflow() {
        let source = format!(
            "class Module = () ( run = ( ^{}1 ) ) : ()",
            "<a: ".repeat(100_000)
        );
        let Err(diagnostics) = crate::parse(&source) else {
            panic!("nesting past the limit is refused");
        };
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message == "pattern nested too deeply"));
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

/// §4.1: `self`, `super`, `outer`, `true`, `false` and `nil` are reserved
/// words, so none of them may be declared or sent as an ordinary identifier.
#[cfg(test)]
mod reserved_word_tests {
    use indoc::formatdoc;

    const RESERVED_WORDS: [&str; 6] = ["self", "super", "outer", "true", "false", "nil"];

    /// The reserved-word diagnostics `member` produces as the one member of a
    /// module's root class.
    fn reserved_word_errors(member: &str) -> Vec<String> {
        let source = formatdoc! {"
            class Root = Object new () (
              {member}
            ) : ()
        "};
        match crate::parse(&source) {
            Ok(_) => Vec::new(),
            Err(diagnostics) => diagnostics
                .into_iter()
                .map(|diagnostic| diagnostic.message)
                .filter(|message| message.contains("reserved word"))
                .collect(),
        }
    }

    fn assert_refused(member: &str, word: &str, role: &str) {
        let errors = reserved_word_errors(member);
        let expected = format!("`{word}` is a reserved word and cannot {role}");
        assert!(
            errors.iter().any(|message| message.contains(&expected)),
            "{member}: expected {expected:?}, got {errors:?}"
        );
    }

    #[test]
    fn a_reserved_word_cannot_name_a_parameter() {
        for word in RESERVED_WORDS {
            assert_refused(&format!("foo: {word} = ( ^0 )"), word, "name a parameter");
            assert_refused(&format!("+ {word} = ( ^0 )"), word, "name a parameter");
        }
    }

    #[test]
    fn a_reserved_word_cannot_name_a_method() {
        for word in RESERVED_WORDS {
            assert_refused(&format!("{word} = ( ^0 )"), word, "be a message selector");
        }
    }

    #[test]
    fn a_reserved_word_cannot_name_a_slot_or_a_temporary() {
        for word in RESERVED_WORDS {
            assert_refused(
                &format!("foo = ( | {word} | ^0 )"),
                word,
                "name a slot or a temporary",
            );
        }
    }

    #[test]
    fn a_reserved_word_cannot_name_a_block_parameter() {
        for word in RESERVED_WORDS {
            assert_refused(
                &format!("foo = ( ^[:{word} | 0] )"),
                word,
                "name a block parameter",
            );
        }
    }

    #[test]
    fn a_reserved_word_cannot_name_a_class() {
        for word in RESERVED_WORDS {
            assert_refused(
                &format!("class {word} = Object new () ()"),
                word,
                "name a class",
            );
        }
    }

    #[test]
    fn a_reserved_word_cannot_be_assigned() {
        for word in RESERVED_WORDS {
            assert_refused(&format!("foo = ( {word}:: 3 )"), word, "be assigned");
        }
    }

    #[test]
    fn a_reserved_word_cannot_be_a_unary_selector() {
        for word in RESERVED_WORDS {
            assert_refused(
                &format!("foo = ( ^3 {word} )"),
                word,
                "be a message selector",
            );
            assert_refused(
                &format!("foo = ( ^3 printString; {word} )"),
                word,
                "be a message selector",
            );
        }
    }

    /// The legal uses stay legal: each as an expression, `super` and `outer`
    /// as a send's receiver, and a keyword that merely begins with one.
    #[test]
    fn the_legal_uses_of_the_reserved_words_are_not_refused() {
        for member in [
            "foo = ( ^self )",
            "foo = ( ^super foo )",
            "foo = ( ^outer Root foo )",
            "foo = ( ^{true. false. nil} )",
            "foo = ( ^3 ifNil: [nil] ifNotNil: [:value | value] )",
            "selfish: selfless = ( ^selfless )",
        ] {
            assert_eq!(
                reserved_word_errors(member),
                Vec::<String>::new(),
                "{member}"
            );
        }
    }
}

/// §4.1 reserves six words and no others, and the grammar is a PEG with no
/// separate scanner (§4.2), so `class`, the access modifiers and `lazy` are
/// names wherever the grammar takes an `identifier`, and are taken as what
/// they introduce only where the production that starts with them matches.
#[cfg(test)]
mod unreserved_word_tests {
    use super::*;
    use indoc::formatdoc;

    const WORDS: [&str; 5] = ["class", "public", "protected", "private", "lazy"];
    const ACCESS_MODIFIERS: [&str; 3] = ["public", "protected", "private"];

    fn parse_sides(instance_side: &str, class_side: &str) -> Result<Module, Vec<String>> {
        let source = formatdoc! {"
            class Root = Object new () (
              {instance_side}
            ) : (
              {class_side}
            )
        "};
        crate::parse(&source).map_err(|diagnostics| {
            diagnostics
                .into_iter()
                .map(|diagnostic| diagnostic.message)
                .collect()
        })
    }

    fn assert_accepted(instance_side: &str) {
        if let Err(errors) = parse_sides(instance_side, "") {
            panic!("{instance_side}: {errors:?}");
        }
    }

    fn assert_refused(instance_side: &str) {
        assert!(
            parse_sides(instance_side, "").is_err(),
            "{instance_side} parsed"
        );
    }

    #[test]
    fn every_word_is_a_name_in_an_expression() {
        for word in WORDS {
            for member in [
                format!("read = ( ^{word} )"),
                format!("write = ( {word}:: 3 )"),
                format!("unary = ( ^3 {word} )"),
                format!("cascade = ( ^3 printString; {word} )"),
                format!("keyword = ( ^3 {word}: 4 )"),
                format!("argument = ( ^3 + {word} )"),
                format!("symbol = ( ^#{word} )"),
                format!("receiver = ( ^{word} {word} )"),
            ] {
                assert_accepted(&member);
            }
        }
    }

    #[test]
    fn every_word_names_a_parameter() {
        for word in WORDS {
            for member in [
                format!("at: {word} = ( ^{word} )"),
                format!("+ {word} = ( ^{word} )"),
                format!("block = ( ^[:{word} | {word}] )"),
                format!("class Box value: {word} = Object new ( | held = {word}. | ) ()"),
            ] {
                assert_accepted(&member);
            }
        }
    }

    #[test]
    fn every_word_names_a_class_and_a_superclass() {
        for word in WORDS {
            assert_accepted(&format!("class {word} = Object new () ()"));
            assert_accepted(&format!("private class {word} = Object new () ()"));
            assert_accepted(&format!(
                "class {word} = Object new () () class Other = {word} new () ()"
            ));
        }
    }

    /// `slotDef → [accessModifier] slotDecl ...` (§6.3.2) takes an access
    /// modifier's word as the modifier whenever it stands first, so one names
    /// a slot or a temporary only behind a modifier; the other two words need
    /// none.
    #[test]
    fn every_word_names_a_slot_and_a_temporary_where_the_grammar_lets_it() {
        for word in WORDS {
            assert_accepted(&format!(
                "class Box = Object new ( | private {word} = 1. | ) ()"
            ));
            assert_accepted(&format!("temporaries = ( | public {word} | ^{word} )"));
        }
        for word in ["class", "lazy"] {
            assert_accepted(&format!("class Box = Object new ( | {word} = 1. | ) ()"));
            assert_accepted(&format!("temporaries = ( | {word} | ^{word} )"));
        }
        for word in ACCESS_MODIFIERS {
            assert_refused(&format!("class Box = Object new ( | {word} = 1. | ) ()"));
            assert_refused(&format!("class Box = Object new ( | {word} | ) ()"));
            assert_refused(&format!("temporaries = ( | {word} | ^1 )"));
            assert_refused(&format!("block = ( ^[:each | | {word} | each] )"));
        }
    }

    /// `methodDecl → [accessModifier] messagePattern ...` (§6.3.4), the same
    /// rule, on both sides.
    #[test]
    fn every_word_names_a_method_where_the_grammar_lets_it() {
        for word in WORDS {
            assert_accepted(&format!("public {word} = ( ^1 )"));
            assert!(
                parse_sides("", &format!("public {word} = ( ^1 )")).is_ok(),
                "class side {word}"
            );
        }
        for word in ["class", "lazy"] {
            assert_accepted(&format!("{word} = ( ^1 )"));
        }
        for word in ACCESS_MODIFIERS {
            assert_refused(&format!("{word} = ( ^1 )"));
            assert!(
                parse_sides("", &format!("{word} = ( ^1 )")).is_err(),
                "class side {word}"
            );
        }
    }

    /// `lazySlotDecl → ((lazyModifier [accessModifier]) | (accessModifier
    /// lazyModifier)) slotDecl ...` (§6.3.3): the modifiers first, then the
    /// name.
    #[test]
    fn every_word_names_a_lazy_slot_where_the_grammar_lets_it() {
        for word in WORDS {
            assert_accepted(&format!("lazy private {word} = 1."));
            assert_accepted(&format!("private lazy {word} ::= 1."));
        }
        for word in ["class", "lazy"] {
            assert_accepted(&format!("lazy {word} = 1."));
        }
        for word in ACCESS_MODIFIERS {
            assert_refused(&format!("lazy {word} = 1."));
        }
    }

    fn instance_side_of(member: &str) -> ClassBody {
        match parse_sides(member, "") {
            Ok(module) => module.root_class.instance_side,
            Err(errors) => panic!("{member}: {errors:?}"),
        }
    }

    fn unary_selector(method: &MethodDeclaration) -> &str {
        match &method.pattern {
            MessagePattern::Unary { selector } => &selector.name,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_declaration_is_what_its_whole_production_matches() {
        // No lazy slot: a private method named `lazy`.
        let body = instance_side_of("private lazy = ( ^1 )");
        assert!(body.slots.is_empty());
        assert_eq!(body.methods[0].access, Some(AccessModifier::Private));
        assert_eq!(unary_selector(&body.methods[0]), "lazy");

        // A lazy slot named `lazy`.
        let body = instance_side_of("lazy lazy = 1.");
        assert!(body.methods.is_empty());
        assert_eq!(body.slots[0].name.name, "lazy");
        assert!(body.slots[0].lazy);

        // A public lazy slot named `public`.
        let body = instance_side_of("lazy public public = 1.");
        assert_eq!(body.slots[0].name.name, "public");
        assert_eq!(body.slots[0].access, Some(AccessModifier::Public));
        assert!(body.slots[0].lazy);

        // A nested class named `public`, and a public method named `class`.
        let body = instance_side_of("class public = Object new () () public class = ( ^1 )");
        assert_eq!(body.nested_classes[0].name.name, "public");
        assert_eq!(body.methods[0].access, Some(AccessModifier::Public));
        assert_eq!(unary_selector(&body.methods[0]), "class");
    }

    /// The one deviation (decided with the user on 2026-09-27): §6.3.4's
    /// `accessModifier` and §6.3.3's `lazyModifier` end in `whitespace`, which
    /// would make `| public= 1. |` a slot named `public` beside an erroneous
    /// `| public = 1. |`. Both are the same tokens here, and both are refused.
    #[test]
    fn a_modifier_needs_no_whitespace_after_it() {
        assert_refused("class Box = Object new ( | public= 1. | ) ()");
        assert_refused("lazy public= 1.");
        assert_accepted("lazy public public= 1.");
    }

    #[test]
    fn an_initializer_ends_with_a_dot() {
        for (member, expected) in [
            (
                "class Box = Object new ( | held = 1 | ) ()",
                "after a slot's initializer",
            ),
            (
                "temporaries = ( | held = 1 | ^held )",
                "after a slot's initializer",
            ),
            ("lazy held = 1", "after a lazy slot's initializer"),
        ] {
            let Err(errors) = parse_sides(member, "") else {
                panic!("{member} parsed");
            };
            assert!(
                errors.iter().any(|message| message.contains(expected)),
                "{member}: {errors:?}"
            );
        }
        assert_accepted("class Box = Object new ( | held. other | ) ()");
    }
}

/// Each rule of the specification's grammar the parser holds a program to,
/// with the forms it accepts and the ones it refuses (`docs/design/frontend.md`,
/// "Grammar").
#[cfg(test)]
mod specification_tests {
    use super::*;
    use indoc::formatdoc;

    fn parse_member(member: &str) -> Result<Module, Vec<String>> {
        parse_module_source(&formatdoc! {"
            class Root = Object new () (
              {member}
            ) : ()
        "})
    }

    fn parse_module_source(source: &str) -> Result<Module, Vec<String>> {
        crate::parse(source).map_err(|diagnostics| {
            diagnostics
                .into_iter()
                .map(|diagnostic| diagnostic.message)
                .collect()
        })
    }

    fn accepted(member: &str) {
        if let Err(errors) = parse_member(member) {
            panic!("{member}: {errors:?}");
        }
    }

    fn refused(member: &str) {
        assert!(parse_member(member).is_err(), "{member} parsed");
    }

    fn expression(source: &str) -> Expression {
        let module = parse_member(&format!("probe = ( ^{source} )"))
            .unwrap_or_else(|errors| panic!("{source}: {errors:?}"));
        let method = &module.root_class.instance_side.methods[0];
        match &method.body.statements[0] {
            Statement::Return(expression) => expression.clone(),
            other => panic!("{other:?}"),
        }
    }

    /// §4.2's `decimalNum → ["-"] digits ...` stands where a literal can, and
    /// `binaryMsg → binarySelector unaryExpression` where a binary message
    /// can: `3 -4` subtracts, `foo: -4` passes a negative number.
    #[test]
    fn a_negative_number_is_one_only_where_a_literal_stands() {
        for source in ["3 -4", "3-4", "3 - 4"] {
            let Expression::Send(send) = expression(source) else {
                panic!("{source}")
            };
            assert_eq!(
                send.message.selector,
                Selector::Binary("-".into()),
                "{source}"
            );
            assert!(
                matches!(&send.message.arguments[0], Expression::Literal(Literal::Integer { text, .. }) if text == "4"),
                "{source}"
            );
        }
        let Expression::Send(send) = expression("3 at: -4") else {
            panic!()
        };
        assert!(matches!(
            &send.message.arguments[0],
            Expression::Literal(Literal::Integer { text, .. }) if text == "-4"
        ));
        let Expression::Send(send) = expression("3 +-4") else {
            panic!()
        };
        assert_eq!(send.message.selector, Selector::Binary("+".into()));
        accepted("probe = ( ^3 - -4 )");
        accepted("probe = ( ^Object new; -4 )");
    }

    /// §5.4.3: a setter keyword begins an expression -- chained, as the
    /// section's rationale gives `w::x::y` -- and nowhere else.
    #[test]
    fn a_setter_keyword_begins_an_expression_and_nothing_else() {
        accepted("probe = ( | a b | a:: b:: 3 )");
        refused("probe = ( ^self foo:: 3 )");
        refused("probe = ( ^self foo; bar:: 3 )");
        refused("foo:: value = ( ^value )");
    }

    /// §5.4.4: `cascadedMessageExpression → primary [msgCascade]`,
    /// `msgCascade → nonEmptyMessages cascadeMsg*`.
    #[test]
    fn a_cascade_follows_a_primary_and_a_message_to_it() {
        accepted("probe = ( ^self foo; bar; + 1; = 2; at: 3 )");
        accepted("probe = ( ^(self) foo; bar )");
        refused("probe = ( ^self; foo )");
        refused("probe = ( ^(self foo); bar )");
        refused("probe = ( foo: 1; bar: 2 )");
    }

    /// §5.9 and §5.10: `super` is followed by a message, `outer` by a class's
    /// name -- and `outer N` alone is the enclosing object, decided with the
    /// user on 2026-09-27.
    #[test]
    fn super_takes_a_message_and_outer_a_name() {
        accepted("probe = ( ^super foo )");
        accepted("probe = ( ^super + 1 )");
        accepted("probe = ( ^super at: 1 )");
        refused("probe = ( ^super )");
        refused("probe = ( ^(super) foo )");
        accepted("probe = ( ^outer Root foo )");
        accepted("probe = ( ^outer Root )");
        refused("probe = ( ^outer )");
        refused("probe = ( ^outer foo: 3 )");
    }

    /// §5.6: `<-:` stands before a message sent to an explicit receiver.
    #[test]
    fn an_eventual_send_stands_before_a_message() {
        accepted("probe = ( ^self <-: foo )");
        accepted("probe = ( ^self <-: + 1 )");
        accepted("probe = ( ^self <-: at: 1 )");
        accepted("probe = ( ^self foo; <-: bar )");
        refused("probe = ( ^<-: foo )");
        refused("probe = ( ^self <-: )");
    }

    /// §5.1.9's pattern literal.
    #[test]
    fn a_pattern_literal_follows_its_grammar() {
        for pattern in [
            "<_>",
            "<3>",
            "<-3>",
            "<#foo>",
            "<'s'>",
            "<{1. 2}>",
            "<x:>",
            "<x: _ y: ?z>",
            "<x: <y: 1>>",
            "<x: <y: <z: 1>>>",
        ] {
            accepted(&format!("probe = ( ^{pattern} )"));
        }
        for pattern in ["<>", "<foo>", "<x: foo>", "<x: 1"] {
            refused(&format!("probe = ( ^{pattern} )"));
        }
    }

    /// §5.1.8's `statements → returnStatement | statementSequence | empty`,
    /// `statementSequence → expression [dot statements]`,
    /// `returnStatement → hat expression [dot]`.
    #[test]
    fn a_dot_ends_a_statement_and_nothing_else() {
        accepted("probe = ( self foo. self bar. )");
        accepted("probe = ( self foo. ^self bar. )");
        accepted("probe = ( )");
        refused("probe = ( . self foo )");
        refused("probe = ( self foo.. self bar )");
        refused("probe = ( ^self foo. self bar )");
        refused("probe = ( [:each | . each] )");
    }

    /// §6.3's `initExprs → [expression (dot expression)* [dot]]`, and
    /// `sideDecl` and `classSideDecl`, which have no separator at all.
    #[test]
    fn dots_in_a_class_stand_between_initializer_expressions_only() {
        accepted("class Box = Object new ( self foo. self bar. ) ()");
        refused("class Box = Object new ( . self foo ) ()");
        refused("class Box = Object new ( self foo.. self bar ) ()");
        refused("class Box = Object new ( self foo self bar ) ()");
        refused("first = ( ^1 ). second = ( ^2 )");
        assert!(parse_module_source(indoc::indoc! {"
            class Root = () () : ( first = ( ^1 ). )
        "})
        .is_err());
    }

    /// §6.1's `inheritanceClause → [inheritancePrefix] unarySelector [message]`.
    #[test]
    fn a_superclass_clause_sends_one_message() {
        accepted("class Box = Object new () ()");
        accepted("class Box = Object + 1 () ()");
        accepted("class Box = Object new: 1 + 2 with: 3 foo () ()");
        accepted("class Box = self Object new () ()");
        refused("class Box = Object new foo () ()");
        refused("class Box = Object new + 1 () ()");
        // The receiver is a unary selector: a name, never a literal or a
        // reserved word, and `outer`'s class is a name too.
        refused("class Box = 3 new () ()");
        refused("class Box = nil new () ()");
        refused("class Box = outer 3 Object new () ()");
    }

    /// §6.3's `classBody → classHeader sideDecl [classSideDecl]`, and §6.2's
    /// `mixinAppSuffix → (mixinOperator inheritanceClause)+ (dot | classBody)`.
    #[test]
    fn a_class_has_an_instance_side_and_may_have_a_class_side() {
        accepted("class Box = Object new () ()");
        accepted("class Box = Object new () () : ()");
        accepted("class Box = Object new <: Object.");
        refused("class Box = Object new ()");
    }

    /// §8.1's `compilationUnit → languageId toplevelClass`,
    /// `toplevelClass → classCategory classDeclaration`, `classCategory → [string]`.
    #[test]
    fn a_module_has_an_optional_category_and_no_access_modifier() {
        assert!(parse_module_source("Newspeak3\nclass Root = () ()\n").is_ok());
        assert!(parse_module_source("'T'\nclass Root = () ()\n").is_ok());
        assert!(parse_module_source("Newspeak3\n'T'\npublic class Root = () ()\n").is_err());
        assert!(parse_module_source("'T'\npublic class Root = () ()\n").is_err());
    }

    /// §8.1's `languageId`, which ns-next makes optional
    /// (`docs/design/frontend.md`, "The module header"). `class` and the
    /// access modifiers are unreserved words (§4.1), so each is a language id
    /// exactly when a `toplevelClass` follows it, and opens the declaration
    /// otherwise.
    #[test]
    fn a_module_may_leave_its_language_id_out() {
        let language_id_of = |source: &str| {
            parse_module_source(source)
                .unwrap_or_else(|errors| panic!("{source}: {errors:?}"))
                .language_id
        };
        assert_eq!(language_id_of("class Root = () ()\n"), "");
        assert_eq!(language_id_of("'T'\nclass Root = () ()\n"), "");
        assert_eq!(language_id_of("class class = () ()\n"), "");
        assert_eq!(language_id_of("class class Root = () ()\n"), "class");
        assert_eq!(language_id_of("class\n'T'\nclass Root = () ()\n"), "class");
        assert_eq!(language_id_of("public class Root = () ()\n"), "public");
        assert_eq!(
            language_id_of("Newspeak3\nclass Root = () ()\n"),
            "Newspeak3"
        );
        assert_eq!(
            language_id_of("Newspeak3\n'T'\nclass Root = () ()\n"),
            "Newspeak3"
        );
    }

    /// A name is expected where the grammar has an `identifier`.
    #[test]
    fn a_declaration_names_what_it_declares() {
        assert!(parse_module_source("class 3 = () ()\n").is_err());
        refused("foo: 3 = ( ^1 )");
        refused("+ 3 = ( ^1 )");
        refused("probe = ( ^[:3 | 1] )");
    }

    /// §6.3.4's `binaryMsgPattern → binarySelector slotDecl`, any binary
    /// selector, `||` and a negative-looking `-` included.
    #[test]
    fn every_binary_selector_declares_a_binary_method() {
        for selector in ["+", "=", "<", ">", "|", "||", "->", "-", "<=", "\\\\"] {
            accepted(&format!("{selector} other = ( ^other )"));
        }
    }

    /// §5.1.1 gives a radix number a value only when its radix is between 2
    /// and 36 and every digit is below it; the canonical parser refuses the
    /// rest while it parses, and so does this one. A fraction or an exponent
    /// makes a number non-integral, a radix one included.
    #[test]
    fn a_radix_number_denotes_a_number() {
        for number in ["2r1010", "36rZ", "16r1.8", "2r1e3", "16r-1F", "8r7.7e-2"] {
            accepted(&format!("probe = ( ^{number} )"));
        }
        for number in ["2r12", "16rG", "2r1.2", "1r0", "37r1", "0r0"] {
            refused(&format!("probe = ( ^{number} )"));
        }
        for (number, non_integral) in [
            ("16r1F", false),
            ("16r1.8", true),
            ("2r1e3", true),
            ("1e3", true),
        ] {
            let integral = matches!(
                expression(number),
                Expression::Literal(Literal::Integer { .. })
            );
            assert_eq!(integral, !non_integral, "{number}");
        }
    }

    /// §6's `classDeclaration` tries `messagePattern equalSign` before
    /// `empty equalSign`, so an `=` with a parameter and another `=` after it
    /// is a binary factory, not the end of the header.
    #[test]
    fn a_binary_factory_may_be_named_equals() {
        let module = parse_module_source("class Pair = other = Object new () ()\n")
            .unwrap_or_else(|errors| panic!("{errors:?}"));
        let class = &module.root_class;
        assert!(
            matches!(
                &class.factory,
                Some(MessagePattern::Binary { selector, parameter })
                    if selector.name == "=" && parameter.name == "other"
            ),
            "{:?}",
            class.factory
        );
        assert_eq!(
            class
                .superclass
                .as_ref()
                .map(|clause| clause.receiver.name.as_str()),
            Some("Object")
        );
    }

    /// §6.3.2's `slotDecls → seqSlotDecls | simSlotDecls` in a class header
    /// and in a code body (§5.1.8's `temporaries → slotDecls`) alike: an
    /// empty sequential section may be written `||`, and a simultaneous one
    /// declares slots between `||` and `||`.
    #[test]
    fn a_slot_section_is_sequential_or_simultaneous_everywhere() {
        accepted("class Box = Object new ( || ) ( probe = ( || ^1 ) )");
        accepted("class Box = Object new ( | | ) ( probe = ( | | ^1 ) )");
        accepted("class Box = Object new ( || a = 1. b = 2. || ) ()");
        let module = parse_member("probe = ( || c = 3. d = 4. || ^c + d )")
            .unwrap_or_else(|errors| panic!("{errors:?}"));
        assert_eq!(
            module.root_class.instance_side.methods[0]
                .body
                .temporaries
                .len(),
            2
        );
    }

    /// §6.3.2: a declaration remembers which clause it stood in, since a
    /// simultaneous clause initializes differently. A lone `||` is still the
    /// empty sequential section, and a lazy slot belongs to no clause.
    #[test]
    fn a_slot_declaration_records_whether_its_clause_is_simultaneous() {
        let source = indoc::indoc! {"
            class Probe = Object new (|| first = second. second = 2. ||) (
                lazy late = 3.
                public sequential = ( | one = 1. | ^one )
                public simultaneous = ( || one = two. two = 2. || ^one )
                public empty = ( || ^nil )
            ) : ()
        "};
        let module = crate::parse(source).expect("parse");
        let body = &module.root_class.instance_side;
        let header: Vec<bool> = body
            .slots
            .iter()
            .filter(|slot| !slot.lazy)
            .map(|slot| slot.simultaneous)
            .collect();
        assert_eq!(header, vec![true, true]);
        assert!(body
            .slots
            .iter()
            .filter(|slot| slot.lazy)
            .all(|slot| !slot.simultaneous));
        let temporaries = |selector_name: &str| -> Vec<bool> {
            body.methods
                .iter()
                .find(|method| match &method.pattern {
                    MessagePattern::Unary { selector } => selector.name == selector_name,
                    _ => false,
                })
                .map(|method| {
                    method
                        .body
                        .temporaries
                        .iter()
                        .map(|temporary| temporary.simultaneous)
                        .collect()
                })
                .unwrap_or_default()
        };
        assert_eq!(temporaries("sequential"), vec![false]);
        assert_eq!(temporaries("simultaneous"), vec![true, true]);
        assert_eq!(temporaries("empty"), Vec::<bool>::new());
    }

    /// §6's `typeParameterDecl → langleBracket lbracket identifier (comma
    /// identifier)* rbracket rangleBracket`, dropped like every type.
    #[test]
    fn a_class_declares_its_type_parameters_as_the_specification_writes_them() {
        accepted("class Map <[K, V]> new: capacity <Integer> = Object new () ()");
        accepted("class Set <[E]> = Object new () ()");
        accepted("class List[E] = Object new () ()");
        refused("class Map <[3]> = Object new () ()");
        refused("class Map <[K V]> = Object new () ()");
        refused("class Map <[K,]> = Object new () ()");
        refused("class Map <[]> = Object new () ()");
    }
}
