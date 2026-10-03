//! What the resolver knows about each name, as LSP semantic tokens.

use std::collections::HashMap;

use lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};
use ns_runtime::Binding;
use ns_syntax::ast::{self, Expression, Statement};
use ns_syntax::{Span, Token, TokenKind};

use crate::documents::{utf16_length, LineIndex};

/// The token types of the legend, in legend order (`legend`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenCategory {
    Parameter,
    Variable,
    Property,
    Class,
    Method,
    Keyword,
}

/// One classified name: where it is, what it is, and whether this is its
/// declaration and whether it can never be assigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Classified {
    pub span: Span,
    pub category: TokenCategory,
    pub declaration: bool,
    pub readonly: bool,
}

impl TokenCategory {
    fn legend_index(self) -> u32 {
        match self {
            TokenCategory::Parameter => 0,
            TokenCategory::Variable => 1,
            TokenCategory::Property => 2,
            TokenCategory::Class => 3,
            TokenCategory::Method => 4,
            TokenCategory::Keyword => 5,
        }
    }
}

const DECLARATION_BIT: u32 = 1;
const READONLY_BIT: u32 = 2;

/// The legend the server announces; `TokenCategory::legend_index` and the
/// two bits above index into it. The JetBrains plugin maps these strings.
pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::PARAMETER,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::PROPERTY,
            SemanticTokenType::CLASS,
            SemanticTokenType::METHOD,
            SemanticTokenType::KEYWORD,
        ],
        token_modifiers: vec![
            SemanticTokenModifier::DECLARATION,
            SemanticTokenModifier::READONLY,
        ],
    }
}

/// LSP's relative encoding of `classified`: sorted by position, each token's
/// line relative to the previous one's, its start relative to the previous
/// one's on the same line, both in UTF-16. A token that overlaps the one
/// before it is dropped, since the protocol has no way to express overlap.
pub fn encode(classified: &[Classified], text: &str, line_index: &LineIndex) -> Vec<SemanticToken> {
    let mut sorted = classified.to_vec();
    sorted.sort_by_key(|entry| (entry.span.start, entry.span.end));
    let mut encoded = Vec::with_capacity(sorted.len());
    let mut previous_line = 0;
    let mut previous_start = 0;
    let mut previous_end = 0;
    for entry in sorted {
        if !encoded.is_empty() && entry.span.start < previous_end {
            continue;
        }
        let position = line_index.position(text, entry.span.start);
        let delta_line = position.line - previous_line;
        let delta_start = if delta_line == 0 {
            position.character - previous_start
        } else {
            position.character
        };
        let mut modifiers = 0;
        if entry.declaration {
            modifiers |= DECLARATION_BIT;
        }
        if entry.readonly {
            modifiers |= READONLY_BIT;
        }
        encoded.push(SemanticToken {
            delta_line,
            delta_start,
            length: utf16_length(text, entry.span),
            token_type: entry.category.legend_index(),
            token_modifiers_bitset: modifiers,
        });
        previous_line = position.line;
        previous_start = position.character;
        previous_end = entry.span.end;
    }
    encoded
}

/// What one class declares, each side apart, under the full selector. The
/// resolver searches both sides together (`own_member_names` in
/// `crates/ns-runtime/src/resolver/classes.rs`) and records only the depth a
/// send binds at. At depth 0 the send reaches the object the body holding it
/// runs on: an instance from an instance-side body, the class object from a
/// class-side method. Further out it reaches an enclosing object, an instance:
/// a class side holds methods only (the parser's G3), so every nested class
/// is declared on an instance side. A name both sides declare with different
/// kinds is coloured by the side reached. A name one side declares twice is a
/// resolver error; the first declaration wins here, so the colour is stable
/// while the error is shown.
struct ClassMembers {
    instance_side: SideMembers,
    class_side: SideMembers,
}

/// What one side of a class declares: one level of the chain a send's depth
/// indexes.
#[derive(Default)]
struct SideMembers {
    members: HashMap<String, MemberKind>,
}

#[derive(Clone, Copy)]
enum MemberKind {
    Slot { readonly: bool },
    SlotSetter,
    NestedClass,
    Method,
}

impl ClassMembers {
    fn of(declaration: &ast::ClassDeclaration) -> ClassMembers {
        let mut class_side = declaration
            .class_side
            .as_ref()
            .map_or_else(SideMembers::default, SideMembers::of);
        // §6: a declaration without a factory signature has the factory `new`;
        // §6.3 makes it a member of the metaclass mixin the class side becomes.
        let factory = declaration
            .factory
            .as_ref()
            .map_or_else(|| "new".to_string(), selector_of);
        class_side
            .members
            .entry(factory)
            .or_insert(MemberKind::Method);
        ClassMembers {
            instance_side: SideMembers::of(&declaration.instance_side),
            class_side,
        }
    }
}

impl SideMembers {
    fn of(body: &ast::ClassBody) -> SideMembers {
        let mut members = HashMap::new();
        for slot in &body.slots {
            let readonly = slot.mutability == ast::SlotMutability::Immutable;
            members
                .entry(slot.name.name.clone())
                .or_insert(MemberKind::Slot { readonly });
            if !readonly {
                members
                    .entry(format!("{}:", slot.name.name))
                    .or_insert(MemberKind::SlotSetter);
            }
        }
        for method in &body.methods {
            members
                .entry(selector_of(&method.pattern))
                .or_insert(MemberKind::Method);
        }
        for nested in &body.nested_classes {
            members
                .entry(nested.name.name.clone())
                .or_insert(MemberKind::NestedClass);
        }
        SideMembers { members }
    }
}

/// A message pattern's full selector: `at:put:` for a keyword pattern, whose
/// parts the parser stores without their colon.
fn selector_of(pattern: &ast::MessagePattern) -> String {
    match pattern {
        ast::MessagePattern::Unary { selector } | ast::MessagePattern::Binary { selector, .. } => {
            selector.name.clone()
        }
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|(keyword, _)| format!("{}:", keyword.name))
            .collect(),
    }
}

fn selector_name(selector: &ast::Selector) -> &str {
    match selector {
        ast::Selector::Unary(name) | ast::Selector::Binary(name) | ast::Selector::Keyword(name) => {
            name
        }
    }
}

#[derive(Clone, Copy)]
enum LocalKind {
    Parameter,
    Temporary { readonly: bool },
}

struct Classifier<'a> {
    bindings: &'a HashMap<Span, Binding>,
    tokens: &'a [Token],
    /// The local scopes entered, innermost last, each a list of names in
    /// declaration order.
    scopes: Vec<Vec<(String, LocalKind)>>,
    output: Vec<Classified>,
}

/// Every name in `module` the resolver can place; see this task's mapping.
pub fn classify(
    module: &ast::Module,
    bindings: &HashMap<Span, Binding>,
    tokens: &[Token],
) -> Vec<Classified> {
    let mut classifier = Classifier {
        bindings,
        tokens,
        scopes: Vec::new(),
        output: Vec::new(),
    };
    classifier.class(&module.root_class, &[]);
    classifier.output
}

impl Classifier<'_> {
    fn emit(&mut self, span: Span, category: TokenCategory, declaration: bool, readonly: bool) {
        self.output.push(Classified {
            span,
            category,
            declaration,
            readonly,
        });
    }

    /// `enclosing` is the chain around this declaration, innermost first --
    /// the resolver's `class_chain` without this class's own level, each
    /// level the side the class nested in it is declared on.
    fn class(&mut self, declaration: &ast::ClassDeclaration, enclosing: &[&SideMembers]) {
        let own = ClassMembers::of(declaration);
        let mut chain: Vec<&SideMembers> = vec![&own.instance_side];
        chain.extend_from_slice(enclosing);
        let mut class_side_chain: Vec<&SideMembers> = vec![&own.class_side];
        class_side_chain.extend_from_slice(enclosing);

        self.emit(declaration.name.span, TokenCategory::Class, true, false);
        // A module root's clauses bind nothing; a nested class's receivers
        // resolve against the chain enclosing it. `resolve_superclass_clause`
        // records a binding at the receiver's span for every prefix, not
        // only a bare one: `self`/`outer N` record `LexicalSend`, which
        // `member_use` looks up the same way a bare receiver's does, and
        // `super` records `SuperSend`, classified as a `Method` directly --
        // the same way a `super` send's selector is, since the clause reads
        // the applying class's own binding, never the enclosing chain's.
        if !enclosing.is_empty() {
            for clause in declaration
                .superclass
                .iter()
                .chain(&declaration.mixin_applications)
            {
                match &clause.prefix {
                    Some(ast::SuperclassPrefix::SuperPrefix) => {
                        self.emit(clause.receiver.span, TokenCategory::Method, false, false);
                    }
                    _ => {
                        self.member_use(clause.receiver.span, &clause.receiver.name, enclosing);
                    }
                }
            }
        }

        // Slot initializers, initializer expressions and the clauses'
        // message arguments run in the instance initializer, where the
        // factory's parameters are locals.
        let factory_parameters: Vec<&ast::Identifier> = declaration
            .factory
            .as_ref()
            .map(pattern_parameters)
            .unwrap_or_default();
        self.enter_scope(&factory_parameters, &[]);
        for slot in declaration
            .instance_side
            .slots
            .iter()
            .filter(|slot| !slot.lazy)
        {
            let readonly = slot.mutability == ast::SlotMutability::Immutable;
            self.emit(slot.name.span, TokenCategory::Property, true, readonly);
            if let Some(initializer) = &slot.initializer {
                self.expression(initializer, &chain);
            }
        }
        for statement in &declaration.instance_side.initializer_expressions {
            self.statement(statement, &chain);
        }
        for clause in declaration
            .superclass
            .iter()
            .chain(&declaration.mixin_applications)
        {
            // The clause's messages go to the class its receiver names -- a
            // message to an object like any other.
            for message in &clause.messages {
                self.selector(clause.receiver.span, message, TokenCategory::Method, false);
                for argument in &message.arguments {
                    self.expression(argument, &chain);
                }
            }
        }
        self.scopes.pop();

        // §6: the factory's parameters are not in scope in the class body,
        // where a lazy slot is declared; the resolver resolves its
        // initializer as a unary method's body, and so does this walk.
        for slot in declaration
            .instance_side
            .slots
            .iter()
            .filter(|slot| slot.lazy)
        {
            if let Some(modifier) = self.lazy_modifier(slot) {
                self.emit(modifier, TokenCategory::Keyword, false, false);
            }
            let readonly = slot.mutability == ast::SlotMutability::Immutable;
            self.emit(slot.name.span, TokenCategory::Property, true, readonly);
            if let Some(initializer) = &slot.initializer {
                self.enter_scope(&[], &[]);
                self.expression(initializer, &chain);
                self.scopes.pop();
            }
        }

        if let Some(class_side) = &declaration.class_side {
            for slot in &class_side.slots {
                let readonly = slot.mutability == ast::SlotMutability::Immutable;
                self.emit(slot.name.span, TokenCategory::Property, true, readonly);
            }
        }
        let mut sides = vec![(&declaration.instance_side, &chain)];
        if let Some(class_side) = &declaration.class_side {
            sides.push((class_side, &class_side_chain));
        }
        for &(body, side_chain) in &sides {
            for method in &body.methods {
                self.method(method, side_chain);
            }
        }
        for &(body, side_chain) in &sides {
            for nested in &body.nested_classes {
                self.class(nested, side_chain);
            }
        }
    }

    fn method(&mut self, method: &ast::MethodDeclaration, chain: &[&SideMembers]) {
        match &method.pattern {
            ast::MessagePattern::Unary { selector }
            | ast::MessagePattern::Binary { selector, .. } => {
                self.emit(selector.span, TokenCategory::Method, true, false);
            }
            ast::MessagePattern::Keyword { parts } => {
                for (keyword, _) in parts {
                    self.emit(keyword.span, TokenCategory::Method, true, false);
                }
            }
        }
        let parameters = pattern_parameters(&method.pattern);
        self.body(&parameters, &method.body, chain);
    }

    fn body(
        &mut self,
        parameters: &[&ast::Identifier],
        body: &ast::CodeBody,
        chain: &[&SideMembers],
    ) {
        self.enter_scope(parameters, &body.temporaries);
        for temporary in &body.temporaries {
            if let Some(initializer) = &temporary.initializer {
                self.expression(initializer, chain);
            }
        }
        for statement in &body.statements {
            self.statement(statement, chain);
        }
        self.scopes.pop();
    }

    /// Pushes a scope and emits each parameter's and temporary's declaration.
    fn enter_scope(
        &mut self,
        parameters: &[&ast::Identifier],
        temporaries: &[ast::SlotDeclaration],
    ) {
        let mut scope = Vec::new();
        for parameter in parameters {
            self.emit(parameter.span, TokenCategory::Parameter, true, false);
            scope.push((parameter.name.clone(), LocalKind::Parameter));
        }
        for temporary in temporaries {
            let readonly = temporary.mutability == ast::SlotMutability::Immutable;
            self.emit(temporary.name.span, TokenCategory::Variable, true, readonly);
            scope.push((
                temporary.name.name.clone(),
                LocalKind::Temporary { readonly },
            ));
        }
        self.scopes.push(scope);
    }

    fn statement(&mut self, statement: &Statement, chain: &[&SideMembers]) {
        match statement {
            Statement::Expression(expression) | Statement::Return(expression) => {
                self.expression(expression, chain)
            }
        }
    }

    fn expression(&mut self, expression: &Expression, chain: &[&SideMembers]) {
        match expression {
            Expression::Identifier(identifier) => self.identifier(identifier, chain),
            Expression::Literal(_) => {}
            Expression::Send(send) => self.send(send, chain),
            Expression::Cascade(cascade) => {
                let receiver = expression_span(&cascade.receiver);
                for message in std::iter::once(&cascade.first).chain(&cascade.rest) {
                    self.explicit_receiver_selector(&cascade.receiver, receiver, message);
                }
                self.expression(&cascade.receiver, chain);
                for message in std::iter::once(&cascade.first).chain(&cascade.rest) {
                    for argument in &message.arguments {
                        self.expression(argument, chain);
                    }
                }
            }
            Expression::Assign { target, value, .. } => {
                self.setter_target(target, chain);
                self.expression(value, chain);
            }
            Expression::Block(block) => {
                let parameters: Vec<&ast::Identifier> = block.parameters.iter().collect();
                self.body(&parameters, &block.body, chain);
            }
            Expression::DynamicArray { elements, .. } => {
                for element in elements {
                    self.expression(element, chain);
                }
            }
        }
    }

    fn identifier(&mut self, identifier: &ast::Identifier, chain: &[&SideMembers]) {
        // `outer` is reserved by the lexer, never a member name (per the
        // spec). `resolve_identifier` skips only `self`/`super`, so a bare
        // `outer` with no further message chained after it (the two-hop
        // `outer N m` shape's `outer_class_name` recognizer only matches
        // when `outer N` is itself a receiver) falls through to the
        // ordinary bare-name case and gets a `LexicalSend` binding like any
        // other identifier. The classifier must not colour it regardless.
        if identifier.name == "outer" {
            return;
        }
        match self.bindings.get(&identifier.span) {
            Some(Binding::Local(_) | Binding::OuterLocal { .. }) => {
                let (category, readonly) = self.local(&identifier.name);
                self.emit(identifier.span, category, false, readonly);
            }
            Some(Binding::LexicalSend { .. }) => {
                self.member_use(identifier.span, &identifier.name, chain);
            }
            _ => {}
        }
    }

    fn setter_target(&mut self, target: &ast::Identifier, chain: &[&SideMembers]) {
        // The binding is keyed by the name's span; the colour covers the
        // whole `name::` token, so the colons read as part of the write.
        let span = self.setter_token(target.span);
        match self.bindings.get(&target.span) {
            Some(Binding::Local(_) | Binding::OuterLocal { .. }) => {
                let (category, readonly) = self.local(&target.name);
                self.emit(span, category, false, readonly);
            }
            Some(Binding::LexicalSend { depth }) => {
                let setter = format!("{}:", target.name);
                let (category, readonly) = member_category(chain, *depth, &setter);
                self.emit(span, category, false, readonly);
            }
            _ => {}
        }
    }

    /// The `lazy` of a lazy slot declaration. The lexer has no token of its
    /// own for it -- it is an identifier the parser recognizes by its text
    /// and position (`at_lazy_slot`) -- so the plugin's lexer, which mirrors
    /// the Rust one token by token, cannot colour it either. Before the name
    /// there is only `lazy` and at most one access modifier, in either
    /// order, so the one identifier there is the modifier.
    fn lazy_modifier(&self, slot: &ast::SlotDeclaration) -> Option<Span> {
        let first = self
            .tokens
            .partition_point(|token| token.span.start < slot.span.start);
        self.tokens[first..]
            .iter()
            .take_while(|token| token.span.start < slot.name.span.start)
            .find(|token| token.kind == TokenKind::Identifier)
            .map(|token| token.span)
    }

    /// The `name::` token that starts where `name` does, or `name`'s own span
    /// when the lexer produced no such token.
    fn setter_token(&self, name: Span) -> Span {
        let first = self
            .tokens
            .partition_point(|token| token.span.start < name.start);
        match self.tokens.get(first) {
            Some(token)
                if matches!(token.kind, TokenKind::SetterKeyword)
                    && token.span.start == name.start =>
            {
                token.span
            }
            _ => name,
        }
    }

    fn send(&mut self, send: &ast::Send, chain: &[&SideMembers]) {
        match self.bindings.get(&send.span).copied() {
            Some(Binding::LexicalSend { depth }) => {
                let (category, readonly) =
                    member_category(chain, depth, selector_name(&send.message.selector));
                self.selector(
                    expression_span(&send.receiver),
                    &send.message,
                    category,
                    readonly,
                );
            }
            Some(Binding::SuperSend) => self.selector(
                expression_span(&send.receiver),
                &send.message,
                TokenCategory::Method,
                false,
            ),
            _ => self.explicit_receiver_selector(
                &send.receiver,
                expression_span(&send.receiver),
                &send.message,
            ),
        }
        // Visiting a `self`, `super` or `outer N` receiver emits nothing: the
        // resolver records no binding inside one.
        self.expression(&send.receiver, chain);
        for argument in &send.message.arguments {
            self.expression(argument, chain);
        }
    }

    /// Colours the selector of a message sent to an object -- a send with an
    /// explicit receiver, which the resolver records no binding for -- as a
    /// method, a binary one included: `a + b` is a send like any other.
    /// `outer N` is not such a send: `N` names an enclosing class, and the
    /// send it receives is the one the resolver binds.
    fn explicit_receiver_selector(
        &mut self,
        receiver: &Expression,
        receiver_span: Span,
        message: &ast::MessageSend,
    ) {
        if matches!(receiver, Expression::Identifier(identifier) if identifier.name == "outer") {
            return;
        }
        self.selector(receiver_span, message, TokenCategory::Method, false);
    }

    /// Emits the selector tokens of `message`: inside the message's span, but
    /// outside the receiver's and every argument's. A keyword message's span
    /// starts at its receiver, so the receiver has to be excluded explicitly.
    fn selector(
        &mut self,
        receiver: Span,
        message: &ast::MessageSend,
        category: TokenCategory,
        readonly: bool,
    ) {
        let excluded: Vec<Span> = std::iter::once(receiver)
            .chain(message.arguments.iter().map(expression_span))
            .collect();
        let message = message.span;
        let first = self
            .tokens
            .partition_point(|token| token.span.start < message.start);
        let selector_spans: Vec<Span> = self.tokens[first..]
            .iter()
            .take_while(|token| token.span.end <= message.end)
            .filter(|token| {
                matches!(
                    token.kind,
                    TokenKind::Identifier
                        // §4.1 does not reserve them, so `x public` is a
                        // unary send like any other.
                        | TokenKind::Class
                        | TokenKind::Public
                        | TokenKind::Protected
                        | TokenKind::Private
                        | TokenKind::Keyword
                        | TokenKind::SetterKeyword
                        | TokenKind::BinarySelector
                        // `=`, `<`, `>`, `|` and `||` have tokens of their
                        // own for the declaration syntax, but after a
                        // receiver the parser takes them as binary selectors
                        // (`parse_binary_message`), so they are one here too.
                        | TokenKind::Equals
                        | TokenKind::LessThan
                        | TokenKind::GreaterThan
                        | TokenKind::Bar
                        | TokenKind::DoubleBar
                )
            })
            .filter(|token| !excluded.iter().any(|span| contains(*span, token.span)))
            .map(|token| token.span)
            .collect();
        for span in selector_spans {
            self.emit(span, category, false, readonly);
        }
    }

    fn member_use(&mut self, span: Span, name: &str, chain: &[&SideMembers]) {
        let depth = match self.bindings.get(&span) {
            Some(Binding::LexicalSend { depth }) => *depth,
            _ => return,
        };
        let (category, readonly) = member_category(chain, depth, name);
        self.emit(span, category, false, readonly);
    }

    /// The innermost local of that name in the scopes entered. The resolver
    /// has already decided the name is a local; this only asks which kind.
    fn local(&self, name: &str) -> (TokenCategory, bool) {
        for scope in self.scopes.iter().rev() {
            if let Some((_, kind)) = scope.iter().rev().find(|(local, _)| local == name) {
                return match kind {
                    LocalKind::Parameter => (TokenCategory::Parameter, false),
                    LocalKind::Temporary { readonly } => (TokenCategory::Variable, *readonly),
                };
            }
        }
        (TokenCategory::Variable, false)
    }
}

/// What `name` is in the class `depth` levels out, or a send when that class
/// does not declare it.
fn member_category(chain: &[&SideMembers], depth: u16, name: &str) -> (TokenCategory, bool) {
    match chain
        .get(depth as usize)
        .and_then(|members| members.members.get(name))
    {
        Some(MemberKind::Slot { readonly }) => (TokenCategory::Property, *readonly),
        Some(MemberKind::SlotSetter) => (TokenCategory::Property, false),
        Some(MemberKind::NestedClass) => (TokenCategory::Class, false),
        Some(MemberKind::Method) | None => (TokenCategory::Method, false),
    }
}

fn pattern_parameters(pattern: &ast::MessagePattern) -> Vec<&ast::Identifier> {
    match pattern {
        ast::MessagePattern::Unary { .. } => Vec::new(),
        ast::MessagePattern::Binary { parameter, .. } => vec![parameter],
        ast::MessagePattern::Keyword { parts } => {
            parts.iter().map(|(_, parameter)| parameter).collect()
        }
    }
}

fn contains(outer: Span, inner: Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end && outer.start < outer.end
}

fn expression_span(expression: &Expression) -> Span {
    match expression {
        Expression::Identifier(identifier) => identifier.span,
        Expression::Literal(literal) => literal_span(literal),
        Expression::Send(send) => send.span,
        Expression::Cascade(cascade) => cascade.span,
        Expression::Assign { span, .. } | Expression::DynamicArray { span, .. } => *span,
        Expression::Block(block) => block.span,
    }
}

fn literal_span(literal: &ast::Literal) -> Span {
    match literal {
        ast::Literal::Integer { span, .. }
        | ast::Literal::Float { span, .. }
        | ast::Literal::String { span, .. }
        | ast::Literal::Symbol { span, .. }
        | ast::Literal::Boolean { span, .. }
        | ast::Literal::Nil { span }
        | ast::Literal::Pattern { span, .. } => *span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODULE: &str = indoc::indoc! {"
        class Shapes usingPlatform: platform = Object new (
          | private count ::= 0. private limit = 10. |
        ) (
          class Inner = Object new () ()
          public grow: amount = (
            | step = 1. total |
            total:: amount + step.
            count:: count + total.
            ^limit
          )
          public make = ( ^Inner new )
          public again = ( ^self grow: 3 )
          public nested = ( ^outer Shapes grow: 4 )
          public each = ( ^[:element | element] )
          public inherited = ( ^printString )
        ) : ()
    "};

    /// What the `occurrence`-th (0-based) appearance of `text` in `MODULE`
    /// was classified as, or `None` when nothing was.
    fn classified_at(text: &str, occurrence: usize) -> Option<(TokenCategory, bool, bool)> {
        let start = MODULE
            .match_indices(text)
            .nth(occurrence)
            .unwrap_or_else(|| panic!("`{text}` #{occurrence} is not in the module"))
            .0 as u32;
        let analysis = crate::analysis::analyze(MODULE);
        assert!(analysis.findings.is_empty(), "the module is clean");
        analysis
            .classified
            .iter()
            .find(|classified| classified.span.start == start)
            .map(|classified| {
                (
                    classified.category,
                    classified.declaration,
                    classified.readonly,
                )
            })
    }

    use TokenCategory::{Class, Method, Parameter, Property, Variable};

    #[test]
    fn declarations() {
        assert_eq!(classified_at("Shapes", 0), Some((Class, true, false)));
        assert_eq!(classified_at("platform", 0), Some((Parameter, true, false)));
        assert_eq!(classified_at("count", 0), Some((Property, true, false)));
        assert_eq!(classified_at("limit", 0), Some((Property, true, true)));
        assert_eq!(classified_at("Inner", 0), Some((Class, true, false)));
        assert_eq!(classified_at("grow:", 0), Some((Method, true, false)));
        assert_eq!(classified_at("amount", 0), Some((Parameter, true, false)));
        assert_eq!(classified_at("step", 0), Some((Variable, true, true)));
        assert_eq!(classified_at("total", 0), Some((Variable, true, false)));
        assert_eq!(classified_at("element", 0), Some((Parameter, true, false)));
    }

    #[test]
    fn locals_and_parameters_in_use() {
        assert_eq!(classified_at("total", 1), Some((Variable, false, false)));
        assert_eq!(classified_at("amount", 1), Some((Parameter, false, false)));
        assert_eq!(classified_at("step", 1), Some((Variable, false, true)));
        assert_eq!(classified_at("element", 1), Some((Parameter, false, false)));
    }

    #[test]
    fn slots_in_use_are_properties() {
        // `count::` is the setter send to the mutable slot, `count` its getter.
        assert_eq!(classified_at("count", 1), Some((Property, false, false)));
        assert_eq!(classified_at("count", 2), Some((Property, false, false)));
        assert_eq!(classified_at("limit", 1), Some((Property, false, true)));
    }

    #[test]
    fn a_write_colours_the_colons_with_the_name() {
        let analysis = crate::analysis::analyze(MODULE);
        for setter in ["count::", "total::"] {
            let start = MODULE.find(setter).expect("the write") as u32;
            let classified = analysis
                .classified
                .iter()
                .find(|classified| classified.span.start == start)
                .expect("the write is classified");
            assert_eq!(
                classified.span,
                Span::new(start, start + setter.len() as u32),
                "`{setter}` is coloured as a whole"
            );
        }
    }

    #[test]
    fn a_nested_class_in_use_is_a_class() {
        assert_eq!(classified_at("Inner", 1), Some((Class, false, false)));
    }

    #[test]
    fn self_and_outer_sends_colour_the_selector_not_the_receiver() {
        assert_eq!(classified_at("grow:", 1), Some((Method, false, false)));
        assert_eq!(classified_at("self", 0), None);
        assert_eq!(classified_at("grow:", 2), Some((Method, false, false)));
        assert_eq!(classified_at("outer", 0), None);
        assert_eq!(classified_at("Shapes", 1), None);
    }

    #[test]
    fn inherited_and_predefined_names_are_sends() {
        assert_eq!(
            classified_at("printString", 0),
            Some((Method, false, false))
        );
        // `Object` in the nested class's clause resolves against `Shapes`,
        // which does not declare it.
        assert_eq!(classified_at("Object", 1), Some((Method, false, false)));
    }

    #[test]
    fn a_module_roots_clause_is_not_classified() {
        assert_eq!(classified_at("Object", 0), None);
    }

    #[test]
    fn a_nested_class_clause_classifies_every_prefix_form() {
        // `resolve_superclass_clause` (`crates/ns-runtime/src/resolver/mod.rs`)
        // records a binding at the receiver's span for every prefix, not only
        // a bare one: `self`/`outer Shapes` record `Binding::LexicalSend`, the
        // same as a bare receiver, so `Inner` there is looked up against
        // `Shapes` and found as the nested class it declares; `super` records
        // `Binding::SuperSend`, classified as a `Method` -- the same way a
        // `super` send's selector is -- never looked up against `Shapes` at
        // all.
        let text = indoc::indoc! {"
            class Shapes = Object new () (
              class Inner = Object new () ()
              class ViaSelf = self Inner new () ()
              class ViaOuter = outer Shapes Inner new () ()
              class ViaSuper = super Inner new () ()
            ) : ()
        "};
        let analysis = crate::analysis::analyze(text);
        assert!(analysis.findings.is_empty(), "the module is clean");
        let classified_at = |occurrence: usize| {
            let start = text
                .match_indices("Inner")
                .nth(occurrence)
                .expect("`Inner` occurrence")
                .0 as u32;
            analysis
                .classified
                .iter()
                .find(|classified| classified.span.start == start)
                .map(|classified| {
                    (
                        classified.category,
                        classified.declaration,
                        classified.readonly,
                    )
                })
        };
        assert_eq!(
            classified_at(0),
            Some((Class, true, false)),
            "the declaration"
        );
        assert_eq!(classified_at(1), Some((Class, false, false)), "self Inner");
        assert_eq!(
            classified_at(2),
            Some((Class, false, false)),
            "outer Shapes Inner"
        );
        assert_eq!(
            classified_at(3),
            Some((Method, false, false)),
            "super Inner"
        );
    }

    #[test]
    fn a_bare_outer_receiver_with_no_further_message_is_not_classified() {
        // `outer_class_name` (`crates/ns-runtime/src/resolver/sends.rs`) only
        // recognizes the two-hop `outer N m` shape when the `outer N` send is
        // itself the receiver of a further message. Here `outer Shapes` is
        // the whole return expression, with nothing chained after it, so
        // `resolve_send` falls through to its ordinary-send case and
        // resolves the `outer` identifier like any other bare name --
        // recording a `LexicalSend` for it. The classifier must still not
        // colour it: `outer` is a reserved word, not a member reference.
        let text = indoc::indoc! {"
            class Shapes = Object new () (
              public nested = ( ^outer Shapes )
            ) : ()
        "};
        let analysis = crate::analysis::analyze(text);
        assert!(analysis.findings.is_empty(), "the module is clean");
        let start = text.find("outer Shapes").expect("the expression") as u32;
        assert!(
            !analysis
                .classified
                .iter()
                .any(|classified| classified.span.start == start),
            "a bare `outer` receiver must not be classified"
        );
    }

    #[test]
    fn a_message_to_an_object_colours_its_selector_as_a_method() {
        // `new` in `Inner new` has an explicit receiver and no binding.
        let text = indoc::indoc! {"
            class Shapes = Object new (
              | list = Object new. |
            ) (
              public make = ( ^Object new )
              public sum = ( ^list size + 1 )
              public put = ( ^list at: 1 put: 2 )
              public cascade = ( ^list yourself; hash )
              public compare = ( ^(list size = 2) | true )
              public less = ( ^list size < 3 )
            ) : ()
        "};
        let analysis = crate::analysis::analyze(text);
        assert!(analysis.findings.is_empty(), "the module is clean");
        let category_of = |needle: &str| {
            let start = text.find(needle).expect("the token") as u32;
            analysis
                .classified
                .iter()
                .find(|classified| classified.span.start == start)
                .map(|classified| classified.category)
        };
        assert_eq!(
            category_of("new ("),
            Some(TokenCategory::Method),
            "a superclass clause's message goes to the class it names"
        );
        for selector in ["new )", "size", "at:", "put:", "yourself", "hash"] {
            assert_eq!(
                category_of(selector),
                Some(TokenCategory::Method),
                "`{selector}` is a message to an object"
            );
        }
        for operator in ["+ 1", "= 2", "< 3", "| true"] {
            assert_eq!(
                category_of(operator),
                Some(TokenCategory::Method),
                "`{operator}` is a binary message to an object"
            );
        }
    }

    #[test]
    fn both_modifier_orders_and_both_forms_of_a_lazy_slot() {
        // §6.3.3: `lazy` may stand before or after the access modifier, and
        // a lazy slot may be declared with `=` or `::=`.
        let text = indoc::indoc! {"
            class Holder usingPlatform: platform = Object new () (
              lazy public first = platform.
              public lazy second = first.
              lazy private third ::= 3.
              protected lazy fourth ::= third.
              lazy fifth = 5.
            ) : ()
        "};
        let analysis = crate::analysis::analyze(text);
        assert!(analysis.findings.is_empty(), "the module is clean");
        let classified_at = |needle: &str, occurrence: usize| {
            let start = text
                .match_indices(needle)
                .nth(occurrence)
                .unwrap_or_else(|| panic!("`{needle}` #{occurrence} is not in the module"))
                .0 as u32;
            analysis
                .classified
                .iter()
                .find(|classified| classified.span.start == start)
                .map(|classified| {
                    (
                        classified.span.end - start,
                        classified.category,
                        classified.declaration,
                        classified.readonly,
                    )
                })
        };
        use TokenCategory::Keyword;
        for occurrence in 0..5 {
            assert_eq!(
                classified_at("lazy", occurrence),
                Some((4, Keyword, false, false)),
                "`lazy` #{occurrence} is a keyword"
            );
        }
        assert_eq!(classified_at("public", 0), None, "the lexer colours it");
        assert_eq!(classified_at("first", 0), Some((5, Property, true, true)));
        assert_eq!(classified_at("second", 0), Some((6, Property, true, true)));
        assert_eq!(classified_at("third", 0), Some((5, Property, true, false)));
        assert_eq!(classified_at("fourth", 0), Some((6, Property, true, false)));
        assert_eq!(classified_at("fifth", 0), Some((5, Property, true, true)));
        // The initializers are walked: a slot read there is a slot.
        assert_eq!(classified_at("first", 1), Some((5, Property, false, true)));
        assert_eq!(classified_at("third", 1), Some((5, Property, false, false)));
        // §6: the factory's parameters are not in scope in a lazy slot's
        // initializer, so `platform` there is a send, not the parameter.
        assert_eq!(
            classified_at("platform", 1),
            Some((8, Method, false, false))
        );
    }

    #[test]
    fn the_unreserved_words_are_coloured_as_the_names_they_are() {
        // §4.1 reserves neither `class` nor the access modifiers: behind a
        // modifier one names a slot, and after a receiver it is a send.
        let text = indoc::indoc! {"
            class Holder = Object new (
              | public public = 1. private class ::= 2. |
            ) (
              public read = ( ^public + class )
              public send: other = ( ^other public; private; protected; class )
            ) : ()
        "};
        let analysis = crate::analysis::analyze(text);
        assert!(analysis.findings.is_empty(), "the module is clean");
        let classified_at = |needle: &str, occurrence: usize| {
            let start = text
                .match_indices(needle)
                .nth(occurrence)
                .unwrap_or_else(|| panic!("`{needle}` #{occurrence} is not in the module"))
                .0 as u32;
            analysis
                .classified
                .iter()
                .find(|classified| classified.span.start == start)
                .map(|classified| (classified.category, classified.declaration))
        };
        assert_eq!(classified_at("public", 0), None, "the modifier");
        assert_eq!(classified_at("public", 1), Some((Property, true)));
        assert_eq!(classified_at("class", 1), Some((Property, true)));
        assert_eq!(classified_at("public", 3), Some((Property, false)), "read");
        assert_eq!(classified_at("class", 2), Some((Property, false)), "read");
        for (word, occurrence) in [
            ("public", 5),
            ("private", 1),
            ("protected", 0),
            ("class", 3),
        ] {
            assert_eq!(
                classified_at(word, occurrence),
                Some((Method, false)),
                "`{word}` sent to `other`"
            );
        }
    }

    #[test]
    fn a_name_on_both_sides_is_coloured_by_the_side_the_send_reaches() {
        // The resolver binds each `size` below by depth alone, over both
        // sides together; the object a send reaches at depth 0 is an instance
        // in an instance-side body and the class object in a class-side one,
        // and at depth 1 from `Inner` an instance of `Shapes`.
        let text = indoc::indoc! {"
            class Shapes = Object new (
              | size = 1. |
            ) (
              class Inner = Object new () (
                public peek = ( ^size )
              )
              public read = ( ^size )
            ) : (
              public size = ( ^2 )
              public ask = ( ^size )
            )
        "};
        let analysis = crate::analysis::analyze(text);
        assert!(analysis.findings.is_empty(), "the module is clean");
        let classified_at = |occurrence: usize| {
            let start = text
                .match_indices("size")
                .nth(occurrence)
                .expect("`size` occurrence")
                .0 as u32;
            analysis
                .classified
                .iter()
                .find(|classified| classified.span.start == start)
                .map(|classified| (classified.category, classified.declaration))
        };
        assert_eq!(classified_at(0), Some((Property, true)), "the slot");
        assert_eq!(classified_at(1), Some((Property, false)), "from `Inner`");
        assert_eq!(classified_at(2), Some((Property, false)), "instance side");
        assert_eq!(classified_at(3), Some((Method, true)), "the method");
        assert_eq!(classified_at(4), Some((Method, false)), "class side");
    }

    /// Each occurrence of `needle` in `text` -- a clean module -- with what it
    /// was classified as, in order.
    fn classified_occurrences(text: &str, needle: &str) -> Vec<Option<(TokenCategory, bool)>> {
        let analysis = crate::analysis::analyze(text);
        assert!(
            analysis.findings.is_empty(),
            "the module is clean: {:?}",
            analysis
                .findings
                .iter()
                .map(|finding| &finding.message)
                .collect::<Vec<_>>()
        );
        text.match_indices(needle)
            .map(|(start, _)| {
                analysis
                    .classified
                    .iter()
                    .find(|classified| classified.span.start == start as u32)
                    .map(|classified| (classified.category, classified.declaration))
            })
            .collect()
    }

    #[test]
    fn an_enclosing_level_is_always_an_instance_side() {
        // A class side holds methods only (the parser's G3), so every nested
        // class is declared on an instance side, and its enclosing object is
        // an instance. `size` in `Inner`'s class-side method binds at depth 1,
        // `Shapes`, and reaches its instance side's slot, not its class
        // side's method.
        let text = indoc::indoc! {"
            class Shapes = Object new (
              | size = 1. |
            ) (
              class Inner = Object new () () : (
                public peek = ( ^size )
              )
            ) : (
              public size = ( ^2 )
            )
        "};
        assert_eq!(
            classified_occurrences(text, "size"),
            vec![
                Some((Property, true)),
                Some((Property, false)),
                Some((Method, true))
            ]
        );
    }

    #[test]
    fn a_superclass_clause_receiver_reaches_the_enclosing_instance_side() {
        // §6.1 evaluates the clause as if in an instance method of the
        // enclosing class; the resolver binds `Base` at depth 0 of that
        // chain, `Shapes`, whose instance side declares it as a nested class
        // and whose class side as a method.
        let text = indoc::indoc! {"
            class Shapes = Object new () (
              class Base = Object new () ()
              class Inner = Base new () ()
            ) : (
              public Base = ( ^nil )
            )
        "};
        assert_eq!(
            classified_occurrences(text, "Base"),
            vec![
                Some((Class, true)),
                Some((Class, false)),
                Some((Method, true))
            ]
        );
    }

    #[test]
    fn the_factory_is_a_member_of_the_class_side_only() {
        // §6.3: the primary factory `new` is a member of the class side. The
        // resolver binds both `new` sends at depth 0, `Shapes`; from the
        // instance side the send reaches the instance's slot `new`, from the
        // class side the factory, a method.
        let text = indoc::indoc! {"
            class Shapes = Object new (
              | new = 1. |
            ) (
              public read = ( ^new )
            ) : (
              public make = ( ^new )
            )
        "};
        let occurrences = classified_occurrences(text, "new");
        assert_eq!(
            occurrences[1..],
            [
                Some((Property, true)),
                Some((Property, false)),
                Some((Method, false))
            ],
            "after the module root's own clause"
        );
    }

    #[test]
    fn a_slot_or_method_named_lazy_is_not_a_keyword() {
        let text = indoc::indoc! {"
            class Holder = Object new (
              | lazy = 1. |
            ) (
              public read = ( ^lazy )
            ) : ()
        "};
        let analysis = crate::analysis::analyze(text);
        assert!(analysis.findings.is_empty(), "the module is clean");
        let categories: Vec<TokenCategory> = text
            .match_indices("lazy")
            .filter_map(|(start, _)| {
                analysis
                    .classified
                    .iter()
                    .find(|classified| classified.span.start == start as u32)
                    .map(|classified| classified.category)
            })
            .collect();
        assert_eq!(categories, vec![Property, Property]);
    }

    #[test]
    fn encoding_is_relative_and_in_utf16() {
        let text = "'ä' ab\n  cd";
        let line_index = crate::documents::LineIndex::new(text);
        let classified = [
            Classified {
                span: Span::new(5, 7),
                category: TokenCategory::Variable,
                declaration: true,
                readonly: false,
            },
            Classified {
                span: Span::new(10, 12),
                category: TokenCategory::Method,
                declaration: false,
                readonly: true,
            },
        ];
        let encoded = encode(&classified, text, &line_index);
        assert_eq!(encoded.len(), 2);
        // `ab` is at line 0, UTF-16 column 4 (the umlaut is one unit).
        assert_eq!(
            (
                encoded[0].delta_line,
                encoded[0].delta_start,
                encoded[0].length
            ),
            (0, 4, 2)
        );
        assert_eq!(encoded[0].token_type, 1);
        assert_eq!(encoded[0].token_modifiers_bitset, 0b01);
        // `cd` is on the next line, column 2: the start is absolute again.
        assert_eq!(
            (
                encoded[1].delta_line,
                encoded[1].delta_start,
                encoded[1].length
            ),
            (1, 2, 2)
        );
        assert_eq!(encoded[1].token_type, 4);
        assert_eq!(encoded[1].token_modifiers_bitset, 0b10);
    }

    #[test]
    fn encoding_sorts_and_drops_overlaps() {
        let text = "abcdef";
        let line_index = crate::documents::LineIndex::new(text);
        let variable = |start, end| Classified {
            span: Span::new(start, end),
            category: TokenCategory::Variable,
            declaration: false,
            readonly: false,
        };
        let encoded = encode(
            &[variable(3, 5), variable(0, 2), variable(1, 4)],
            text,
            &line_index,
        );
        let starts: Vec<u32> = encoded.iter().map(|token| token.delta_start).collect();
        assert_eq!(starts, vec![0, 3]);
    }

    #[test]
    fn the_legend_lists_the_categories_in_order() {
        let legend = legend();
        let types: Vec<&str> = legend
            .token_types
            .iter()
            .map(|kind| kind.as_str())
            .collect();
        assert_eq!(
            types,
            vec![
                "parameter",
                "variable",
                "property",
                "class",
                "method",
                "keyword"
            ]
        );
        let modifiers: Vec<&str> = legend
            .token_modifiers
            .iter()
            .map(|modifier| modifier.as_str())
            .collect();
        assert_eq!(modifiers, vec!["declaration", "readonly"]);
    }
}
