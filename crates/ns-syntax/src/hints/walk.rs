//! The walk over a module that hands every section to the hints, with the
//! names its code sees.

use crate::ast::{
    ClassDeclaration, CodeBody, Expression, ExpressionKind, Member, MessagePattern, Module,
    SlotDeclaration, SlotMutability, Statement, StatementKind,
};
use crate::span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SectionKind {
    /// A method's or a closure's code: its temporaries and statements.
    Body,
    /// A class's instance-side header: its slots and initializer
    /// expressions.
    Header,
}

/// Slots and the statements that run after their initializers.
pub(super) struct Section<'tree> {
    pub(super) kind: SectionKind,
    pub(super) slots: &'tree [SlotDeclaration],
    pub(super) statements: &'tree [Statement],
    pub(super) span: Span,
}

/// What a name a section's code reads is bound to, as far as the hints
/// need to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Binding {
    /// A method's, a closure's or a factory's parameter: it never changes.
    Parameter,
    /// A local declared with `=`: it never changes once initialized.
    ReadOnlyLocal,
    /// A local that may change.
    MutableLocal,
}

/// The names declared around a section, the innermost frame last. A class
/// starts afresh: a method sees no enclosing method's names.
#[derive(Default)]
pub(super) struct Scope<'tree> {
    frames: Vec<Vec<(&'tree str, Binding)>>,
}

impl<'tree> Scope<'tree> {
    fn push(&mut self, parameters: Vec<&'tree str>, temporaries: &'tree [SlotDeclaration]) {
        let mut frame: Vec<(&'tree str, Binding)> = parameters
            .into_iter()
            .map(|name| (name, Binding::Parameter))
            .collect();
        frame.extend(temporaries.iter().map(|slot| {
            let binding = if slot.mutability == SlotMutability::Immutable {
                Binding::ReadOnlyLocal
            } else {
                Binding::MutableLocal
            };
            (slot.name.name.as_str(), binding)
        }));
        self.frames.push(frame);
    }

    fn pop(&mut self) {
        self.frames.pop();
    }

    /// What `name` is bound to, the innermost declaration deciding; `None`
    /// for a name no enclosing code declares, which is a send.
    #[expect(dead_code, reason = "used by Task 3")]
    pub(super) fn binding(&self, name: &str) -> Option<Binding> {
        self.frames.iter().rev().find_map(|frame| {
            frame
                .iter()
                .find(|(declared, _)| *declared == name)
                .map(|(_, binding)| *binding)
        })
    }
}

pub(super) trait Visitor<'tree> {
    fn class(&mut self, declaration: &'tree ClassDeclaration);
    fn section(&mut self, section: &Section<'tree>, scope: &Scope<'tree>);
}

pub(super) fn walk<'tree>(module: &'tree Module, visitor: &mut impl Visitor<'tree>) {
    class(&module.root_class, visitor);
}

fn parameters_of(pattern: &MessagePattern) -> Vec<&str> {
    match pattern {
        MessagePattern::Unary { .. } => Vec::new(),
        MessagePattern::Binary { parameter, .. } => vec![parameter.name.name.as_str()],
        MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|part| part.parameter.name.name.as_str())
            .collect(),
    }
}

fn class<'tree>(declaration: &'tree ClassDeclaration, visitor: &mut impl Visitor<'tree>) {
    visitor.class(declaration);
    let instance = &declaration.instance_side;
    // The superclass clauses and the header see the factory's parameters
    // (§6.1); the members do not.
    let mut scope = Scope::default();
    scope.push(
        declaration
            .factory
            .as_ref()
            .map_or_else(Vec::new, parameters_of),
        &[],
    );
    for clause in declaration
        .superclass
        .iter()
        .chain(&declaration.mixin_applications)
    {
        for message in &clause.messages {
            for argument in &message.arguments {
                expression(argument, &mut scope, visitor);
            }
        }
    }
    for slot in &instance.header_slots {
        if let Some(initializer) = &slot.initializer {
            expression(initializer, &mut scope, visitor);
        }
    }
    for statement in &instance.initializer_expressions {
        self::statement(statement, &mut scope, visitor);
    }
    visitor.section(
        &Section {
            kind: SectionKind::Header,
            slots: &instance.header_slots,
            statements: &instance.initializer_expressions,
            span: instance.span,
        },
        &scope,
    );
    for side in std::iter::once(instance).chain(declaration.class_side.as_ref()) {
        for member in &side.members {
            match member {
                Member::Class(nested) => class(nested, visitor),
                Member::LazySlot(slot) => {
                    if let Some(initializer) = &slot.initializer {
                        expression(initializer, &mut Scope::default(), visitor);
                    }
                }
                Member::Method(method) => body(
                    &method.body,
                    parameters_of(&method.pattern),
                    &mut Scope::default(),
                    visitor,
                ),
            }
        }
    }
}

fn body<'tree>(
    code: &'tree CodeBody,
    parameters: Vec<&'tree str>,
    scope: &mut Scope<'tree>,
    visitor: &mut impl Visitor<'tree>,
) {
    scope.push(parameters, &code.temporaries);
    for slot in &code.temporaries {
        if let Some(initializer) = &slot.initializer {
            expression(initializer, scope, visitor);
        }
    }
    for statement in &code.statements {
        self::statement(statement, scope, visitor);
    }
    visitor.section(
        &Section {
            kind: SectionKind::Body,
            slots: &code.temporaries,
            statements: &code.statements,
            span: code.span,
        },
        scope,
    );
    scope.pop();
}

fn statement<'tree>(
    statement: &'tree Statement,
    scope: &mut Scope<'tree>,
    visitor: &mut impl Visitor<'tree>,
) {
    match &statement.kind {
        StatementKind::Expression(inner) | StatementKind::Return(inner) => {
            expression(inner, scope, visitor);
        }
    }
}

fn expression<'tree>(
    expression: &'tree Expression,
    scope: &mut Scope<'tree>,
    visitor: &mut impl Visitor<'tree>,
) {
    match &expression.kind {
        ExpressionKind::Identifier(_) | ExpressionKind::Literal(_) => {}
        // A chain of messages is walked link by link: it nests as deep as
        // the source is long.
        ExpressionKind::Send(send) => {
            let (links, receiver) = send.receiver_chain();
            self::expression(receiver, scope, visitor);
            for link in links.iter().rev() {
                for argument in &link.message.arguments {
                    self::expression(argument, scope, visitor);
                }
            }
        }
        ExpressionKind::Cascade(cascade) => {
            self::expression(&cascade.receiver, scope, visitor);
            for message in std::iter::once(&cascade.first).chain(&cascade.rest) {
                for argument in &message.arguments {
                    self::expression(argument, scope, visitor);
                }
            }
        }
        ExpressionKind::Assign { value, .. } => self::expression(value, scope, visitor),
        ExpressionKind::Block(block) => {
            let parameters = block
                .parameters
                .iter()
                .map(|parameter| parameter.name.name.as_str())
                .collect();
            body(&block.body, parameters, scope, visitor);
        }
        ExpressionKind::DynamicArray { elements, .. } => {
            for element in elements {
                self::expression(element, scope, visitor);
            }
        }
    }
}
