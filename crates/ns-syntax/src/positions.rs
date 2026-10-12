//! A module with its layout erased, so two parses that say the same
//! compare equal however they are laid out.

use crate::ast::*;
use crate::comment::CommentKind;
use crate::span::Span;

impl Module {
    /// This module with every position erased -- spans, delimiters' spans
    /// and block comments' columns zeroed -- so two parses of differently
    /// laid out sources compare equal when they say the same. A block
    /// comment's continuation lines lose as much leading whitespace as its
    /// column, at most what they have: the indentation they keep is the
    /// one relative to `(*`, which a reprint moves with it.
    ///
    /// An empty part reads as absent, because a reprint drops it: a header
    /// slot section without a slot and without a comment between its bars
    /// (`| |`, `||`, `|| ||`) has no `slot_delimiters`, such temporaries
    /// no `temporaries_delimiters`, and a class side without a member and
    /// without a comment is no `class_side`. The attachment counts none of
    /// their tokens (`attach.rs`), so dropping them moves no comment.
    pub fn without_positions(&self) -> Module {
        let mut module = self.clone();
        erase(&mut module);
        module
    }
}

const NOWHERE: Span = Span { start: 0, end: 0 };

/// A node whose positions are still to be erased.
enum Pending<'tree> {
    Class(&'tree mut ClassDeclaration),
    Side(&'tree mut ClassBody),
    Superclass(&'tree mut SuperclassClause),
    Slot(&'tree mut SlotDeclaration),
    Method(&'tree mut MethodDeclaration),
    Body(&'tree mut CodeBody),
    Statement(&'tree mut Statement),
    Expression(&'tree mut Expression),
    Message(&'tree mut MessageSend),
    Block(&'tree mut BlockExpression),
}

/// One walk over the tree, from a stack rather than by recursion: a chain
/// of messages nests as deep as the source is long.
fn erase(module: &mut Module) {
    let Module {
        language_id,
        category,
        root_class,
        comments,
        span,
    } = module;
    *span = NOWHERE;
    if let Some(identifier) = language_id {
        identifier.span = NOWHERE;
    }
    if let Some(category) = category {
        category.span = NOWHERE;
    }
    erase_comments(comments);
    let mut pending = vec![Pending::Class(root_class)];
    while let Some(node) = pending.pop() {
        match node {
            Pending::Class(class) => erase_class(class, &mut pending),
            Pending::Side(side) => erase_side(side, &mut pending),
            Pending::Superclass(clause) => {
                let SuperclassClause {
                    prefix,
                    receiver,
                    receiver_type_arguments,
                    messages,
                    comments,
                    span,
                } = clause;
                if let Some(SuperclassPrefix::Outer(name)) = prefix {
                    name.span = NOWHERE;
                }
                receiver.span = NOWHERE;
                erase_annotation(receiver_type_arguments);
                erase_comments(comments);
                *span = NOWHERE;
                pending.extend(messages.iter_mut().map(Pending::Message));
            }
            Pending::Slot(slot) => {
                let SlotDeclaration {
                    name,
                    type_annotation,
                    initializer,
                    comments,
                    span,
                    ..
                } = slot;
                name.span = NOWHERE;
                erase_annotation(type_annotation);
                erase_comments(comments);
                *span = NOWHERE;
                pending.extend(initializer.as_mut().map(Pending::Expression));
            }
            Pending::Method(method) => {
                let MethodDeclaration {
                    pattern,
                    return_type,
                    where_clause,
                    body,
                    body_delimiters,
                    comments,
                    span,
                    ..
                } = method;
                erase_pattern(pattern);
                erase_annotation(return_type);
                erase_annotation(where_clause);
                erase_delimiters(body_delimiters);
                erase_comments(comments);
                *span = NOWHERE;
                pending.push(Pending::Body(body));
            }
            Pending::Body(body) => {
                let CodeBody {
                    temporaries,
                    temporaries_delimiters,
                    statements,
                    comments,
                    span,
                } = body;
                if temporaries.is_empty() && !holds_a_comment(*temporaries_delimiters, comments) {
                    *temporaries_delimiters = None;
                }
                erase_delimiters(temporaries_delimiters);
                erase_comments(comments);
                *span = NOWHERE;
                pending.extend(temporaries.iter_mut().map(Pending::Slot));
                pending.extend(statements.iter_mut().map(Pending::Statement));
            }
            Pending::Statement(statement) => {
                let Statement {
                    kind,
                    comments,
                    span,
                } = statement;
                erase_comments(comments);
                *span = NOWHERE;
                let (StatementKind::Expression(expression) | StatementKind::Return(expression)) =
                    kind;
                pending.push(Pending::Expression(expression));
            }
            Pending::Expression(expression) => erase_expression(expression, &mut pending),
            Pending::Message(message) => {
                let MessageSend {
                    arguments,
                    type_arguments,
                    comments,
                    span,
                    ..
                } = message;
                erase_annotation(type_arguments);
                erase_comments(comments);
                *span = NOWHERE;
                pending.extend(arguments.iter_mut().map(Pending::Expression));
            }
            Pending::Block(block) => {
                let BlockExpression {
                    parameters,
                    body,
                    delimiters,
                    comments,
                    span,
                } = block;
                parameters.iter_mut().for_each(erase_parameter);
                erase_delimiters(delimiters);
                erase_comments(comments);
                *span = NOWHERE;
                pending.push(Pending::Body(body));
            }
        }
    }
}

fn erase_class<'tree>(class: &'tree mut ClassDeclaration, pending: &mut Vec<Pending<'tree>>) {
    let ClassDeclaration {
        name,
        type_parameters,
        factory,
        factory_return_type,
        factory_where_clause,
        superclass,
        mixin_applications,
        instance_side,
        class_side,
        comments,
        span,
        ..
    } = class;
    if class_side
        .as_ref()
        .is_some_and(|side| side.members.is_empty() && side.comments.is_empty())
    {
        *class_side = None;
    }
    name.span = NOWHERE;
    erase_annotation(type_parameters);
    if let Some(pattern) = factory {
        erase_pattern(pattern);
    }
    erase_annotation(factory_return_type);
    erase_annotation(factory_where_clause);
    erase_comments(comments);
    *span = NOWHERE;
    pending.extend(superclass.as_mut().map(Pending::Superclass));
    pending.extend(mixin_applications.iter_mut().map(Pending::Superclass));
    pending.push(Pending::Side(instance_side));
    pending.extend(class_side.as_mut().map(Pending::Side));
}

fn erase_side<'tree>(side: &'tree mut ClassBody, pending: &mut Vec<Pending<'tree>>) {
    let ClassBody {
        header_slots,
        initializer_expressions,
        members,
        delimiters,
        slot_delimiters,
        comments,
        span,
    } = side;
    if header_slots.is_empty() && !holds_a_comment(*slot_delimiters, comments) {
        *slot_delimiters = None;
    }
    erase_delimiters(delimiters);
    erase_delimiters(slot_delimiters);
    erase_comments(comments);
    *span = NOWHERE;
    pending.extend(header_slots.iter_mut().map(Pending::Slot));
    pending.extend(initializer_expressions.iter_mut().map(Pending::Statement));
    pending.extend(members.iter_mut().map(|member| match member {
        Member::Class(class) => Pending::Class(class),
        Member::LazySlot(slot) => Pending::Slot(slot),
        Member::Method(method) => Pending::Method(method),
    }));
}

fn erase_expression<'tree>(expression: &'tree mut Expression, pending: &mut Vec<Pending<'tree>>) {
    let Expression {
        kind,
        parentheses,
        type_arguments,
        comments,
        span,
        ..
    } = expression;
    for parenthesis in parentheses.iter_mut() {
        parenthesis.delimiters.open = NOWHERE;
        parenthesis.delimiters.close = parenthesis.delimiters.close.map(|_| NOWHERE);
        erase_annotation(&mut parenthesis.type_arguments);
    }
    erase_annotation(type_arguments);
    erase_comments(comments);
    *span = NOWHERE;
    match kind {
        ExpressionKind::Identifier(identifier) => identifier.span = NOWHERE,
        ExpressionKind::Literal(literal) => erase_literal(literal),
        ExpressionKind::Send(send) => {
            let Send {
                receiver,
                message,
                span,
            } = &mut **send;
            *span = NOWHERE;
            pending.push(Pending::Expression(receiver));
            pending.push(Pending::Message(message));
        }
        ExpressionKind::Cascade(cascade) => {
            let Cascade {
                receiver,
                first,
                rest,
                span,
            } = &mut **cascade;
            *span = NOWHERE;
            pending.push(Pending::Expression(receiver));
            pending.push(Pending::Message(first));
            pending.extend(rest.iter_mut().map(Pending::Message));
        }
        ExpressionKind::Assign {
            target,
            value,
            span,
        } => {
            target.span = NOWHERE;
            *span = NOWHERE;
            pending.push(Pending::Expression(value));
        }
        ExpressionKind::Block(block) => pending.push(Pending::Block(block)),
        ExpressionKind::DynamicArray { elements, span } => {
            *span = NOWHERE;
            pending.extend(elements.iter_mut().map(Pending::Expression));
        }
    }
}

fn erase_literal(literal: &mut Literal) {
    match literal {
        Literal::Integer { span, .. }
        | Literal::Float { span, .. }
        | Literal::String { span, .. }
        | Literal::Symbol { span, .. }
        | Literal::Boolean { span, .. }
        | Literal::Nil { span }
        | Literal::Pattern { span, .. } => *span = NOWHERE,
    }
}

fn erase_pattern(pattern: &mut MessagePattern) {
    match pattern {
        MessagePattern::Unary {
            selector,
            type_parameters,
        } => {
            selector.span = NOWHERE;
            erase_annotation(type_parameters);
        }
        MessagePattern::Binary {
            selector,
            parameter,
        } => {
            selector.span = NOWHERE;
            erase_parameter(parameter);
        }
        MessagePattern::Keyword { parts } => {
            for part in parts {
                part.keyword.span = NOWHERE;
                erase_parameter(&mut part.parameter);
            }
        }
    }
}

fn erase_parameter(parameter: &mut Parameter) {
    parameter.name.span = NOWHERE;
    erase_annotation(&mut parameter.type_annotation);
}

fn erase_annotation(annotation: &mut Option<TypeAnnotation>) {
    if let Some(annotation) = annotation {
        annotation.span = NOWHERE;
    }
}

fn erase_delimiters(delimiters: &mut Option<Delimiters>) {
    if let Some(delimiters) = delimiters {
        delimiters.open = NOWHERE;
        delimiters.close = delimiters.close.map(|_| NOWHERE);
    }
}

/// Whether one of `comments` stands between `delimiters`; a part that was
/// never closed holds whatever the recovery gave it.
fn holds_a_comment(delimiters: Option<Delimiters>, comments: &[AttachedComment]) -> bool {
    let Some(Delimiters { open, close }) = delimiters else {
        return false;
    };
    let Some(close) = close else {
        return true;
    };
    comments
        .iter()
        .any(|comment| open.end <= comment.span.start && comment.span.end <= close.start)
}

fn erase_comments(comments: &mut [AttachedComment]) {
    for comment in comments {
        if comment.kind == CommentKind::Block {
            let column = comment.column as usize;
            for line in comment.lines.iter_mut().skip(1) {
                let indentation = line
                    .chars()
                    .take(column)
                    .take_while(|character| character.is_whitespace())
                    .map(char::len_utf8)
                    .sum::<usize>();
                line.replace_range(..indentation, "");
            }
        }
        comment.column = 0;
        comment.span = NOWHERE;
    }
}
