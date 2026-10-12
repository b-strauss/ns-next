//! Expressions and statements lowered to documents, each on one line.

use super::declarations::{after_name, parameter, slot_bar, temporary, type_text};
use super::document::{concat, text, Document};
use crate::ast::{
    BlockExpression, Cascade, Expression, ExpressionKind, Literal, MessageSend, Selector, Send,
    Statement, StatementKind,
};

pub(super) fn statement(statement: &Statement) -> Document {
    match &statement.kind {
        StatementKind::Expression(value) => expression(value),
        StatementKind::Return(value) => concat(vec![text("^"), expression(value)]),
    }
}

pub(super) fn expression(expression: &Expression) -> Document {
    if expression.synthesized {
        return text("");
    }
    let shape = match &expression.kind {
        ExpressionKind::Identifier(identifier) => text(&identifier.name),
        ExpressionKind::Literal(value) => literal(value),
        ExpressionKind::Send(send) => send_chain(send),
        ExpressionKind::Cascade(cascade) => cascaded(cascade),
        ExpressionKind::Assign { target, value, .. } => concat(vec![
            text(format!("{}:: ", target.name)),
            self::expression(value),
        ]),
        ExpressionKind::Block(block) => one_line_block(block),
        ExpressionKind::DynamicArray { elements, .. } => {
            let mut parts = vec![text("{")];
            for (index, element) in elements.iter().enumerate() {
                if index > 0 {
                    parts.push(text(". "));
                }
                parts.push(self::expression(element));
            }
            parts.push(text("}"));
            concat(parts)
        }
    };
    enveloped(expression, shape)
}

/// `shape` with what surrounds it in `expression`: its type arguments,
/// then its parentheses, innermost first, each with its own.
fn enveloped(expression: &Expression, shape: Document) -> Document {
    let mut printed = shape;
    if let Some(arguments) = &expression.type_arguments {
        printed = concat(vec![printed, text(after_name(arguments))]);
    }
    for parenthesis in &expression.parentheses {
        let mut parts = vec![text("("), printed, text(")")];
        if let Some(arguments) = &parenthesis.type_arguments {
            parts.push(text(type_text(arguments)));
        }
        printed = concat(parts);
    }
    printed
}

/// A chain of messages is as deep as the source is long, so it is walked
/// link by link from the receiver it starts at, never by recursing into
/// each receiver.
fn send_chain(send: &Send) -> Document {
    let (links, start) = send.receiver_chain();
    let mut printed = (!start.synthesized).then(|| expression(start));
    for index in (0..links.len()).rev() {
        let sent = message(&links[index].message);
        let combined = match printed {
            Some(receiver) => concat(vec![receiver, text(" "), sent]),
            None => sent,
        };
        // The link outside this one holds this link's envelope as its
        // receiver; the outermost link's envelope is the caller's.
        printed = Some(match index.checked_sub(1) {
            Some(outer) => enveloped(&links[outer].receiver, combined),
            None => combined,
        });
    }
    printed.unwrap_or_else(|| text(""))
}

fn cascaded(cascade: &Cascade) -> Document {
    let mut parts = Vec::new();
    if !cascade.receiver.synthesized {
        parts.push(expression(&cascade.receiver));
        parts.push(text(" "));
    }
    parts.push(message(&cascade.first));
    for each in &cascade.rest {
        parts.push(text("; "));
        parts.push(message(each));
    }
    concat(parts)
}

/// A message without the receiver it is sent to.
pub(super) fn message(message: &MessageSend) -> Document {
    let eventual = if message.eventual { "<-: " } else { "" };
    match &message.selector {
        Selector::Unary(selector) => {
            let type_arguments = message
                .type_arguments
                .as_ref()
                .map(after_name)
                .unwrap_or_default();
            text(format!("{eventual}{selector}{type_arguments}"))
        }
        Selector::Binary(selector) => {
            let mut parts = vec![text(format!("{eventual}{selector} "))];
            parts.extend(message.arguments.iter().map(expression));
            concat(parts)
        }
        Selector::Keyword(selector) => {
            let mut parts = vec![text(eventual)];
            for (index, (keyword, argument)) in selector
                .split_terminator(':')
                .zip(&message.arguments)
                .enumerate()
            {
                if index > 0 {
                    parts.push(text(" "));
                }
                parts.push(text(format!("{keyword}: ")));
                parts.push(expression(argument));
            }
            concat(parts)
        }
    }
}

/// A block on one line, `[:each | each foo]`; the multi-line form and the
/// choice between the two belong to the breaking rules.
fn one_line_block(block: &BlockExpression) -> Document {
    let mut parts = vec![text("[")];
    let body = &block.body;
    let has_body = !body.temporaries.is_empty() || !body.statements.is_empty();
    if !block.parameters.is_empty() {
        let parameters: Vec<String> = block
            .parameters
            .iter()
            .map(|each| format!(":{}", parameter(each)))
            .collect();
        parts.push(text(format!("{} |", parameters.join(" "))));
        if has_body {
            parts.push(text(" "));
        }
    }
    if !body.temporaries.is_empty() {
        let bar = slot_bar(&body.temporaries);
        parts.push(text(format!("{bar} ")));
        for each in &body.temporaries {
            parts.push(temporary(each));
            parts.push(text(" "));
        }
        parts.push(text(bar));
        if !body.statements.is_empty() {
            parts.push(text(" "));
        }
    }
    for (index, each) in body.statements.iter().enumerate() {
        if index > 0 {
            parts.push(text(". "));
        }
        parts.push(statement(each));
    }
    parts.push(text("]"));
    concat(parts)
}

fn literal(literal: &Literal) -> Document {
    match literal {
        Literal::Integer { text: written, .. } | Literal::Float { text: written, .. } => {
            text(written)
        }
        Literal::String { text: written, .. }
        | Literal::Symbol { text: written, .. }
        | Literal::Pattern { text: written, .. } => as_written(written),
        Literal::Boolean { value, .. } => text(if *value { "true" } else { "false" }),
        Literal::Nil { .. } => text("nil"),
    }
}

/// A literal as written: one holding a line break keeps it byte for byte.
pub(super) fn as_written(written: &str) -> Document {
    if written.contains(['\n', '\r']) {
        Document::Verbatim(written.to_string())
    } else {
        text(written)
    }
}
