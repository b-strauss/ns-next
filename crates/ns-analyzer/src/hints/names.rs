//! What code mentions and what it may assign, by name. Every test here is
//! by name alone, never by binding: an `x::` that writes a closure's own
//! `x`, or another class's, still counts as writing `x`. That
//! over-approximates, so it costs a hint at worst, never correctness.

use std::collections::HashSet;

use ns_syntax::ast::{
    CodeBody, Expression, ExpressionKind, MessageSend, Selector, SlotDeclaration, Statement,
    StatementKind,
};

/// One node `each_node` hands over.
enum Node<'tree> {
    Expression(&'tree Expression),
    Message(&'tree MessageSend),
}

/// Hands `visit` every expression and every message in `expression`, the
/// code of its closures included.
fn each_node<'tree>(expression: &'tree Expression, visit: &mut dyn FnMut(Node<'tree>)) {
    visit(Node::Expression(expression));
    match &expression.kind {
        ExpressionKind::Identifier(_) | ExpressionKind::Literal(_) => {}
        // A chain of messages is walked link by link: it nests as deep as
        // the source is long.
        ExpressionKind::Send(send) => {
            let (links, receiver) = send.receiver_chain();
            each_node(receiver, visit);
            for link in links.iter().rev() {
                visit(Node::Message(&link.message));
                for argument in &link.message.arguments {
                    each_node(argument, visit);
                }
            }
        }
        ExpressionKind::Cascade(cascade) => {
            each_node(&cascade.receiver, visit);
            for message in std::iter::once(&cascade.first).chain(&cascade.rest) {
                visit(Node::Message(message));
                for argument in &message.arguments {
                    each_node(argument, visit);
                }
            }
        }
        ExpressionKind::Assign { value, .. } => each_node(value, visit),
        ExpressionKind::Block(block) => each_node_in_body(&block.body, visit),
        ExpressionKind::DynamicArray { elements, .. } => {
            for element in elements {
                each_node(element, visit);
            }
        }
    }
}

fn each_node_in_statement<'tree>(statement: &'tree Statement, visit: &mut dyn FnMut(Node<'tree>)) {
    match &statement.kind {
        StatementKind::Expression(inner) | StatementKind::Return(inner) => each_node(inner, visit),
    }
}

fn each_node_in_slots<'tree>(slots: &'tree [SlotDeclaration], visit: &mut dyn FnMut(Node<'tree>)) {
    for slot in slots {
        if let Some(initializer) = &slot.initializer {
            each_node(initializer, visit);
        }
    }
}

fn each_node_in_body<'tree>(code: &'tree CodeBody, visit: &mut dyn FnMut(Node<'tree>)) {
    each_node_in_slots(&code.temporaries, visit);
    for statement in &code.statements {
        each_node_in_statement(statement, visit);
    }
}

/// Whether `message` is the setter of `name`: the one-keyword selector
/// `name:`.
fn is_setter_of(message: &MessageSend, name: &str) -> bool {
    matches!(&message.selector, Selector::Keyword(selector) if selector.strip_suffix(':') == Some(name))
}

/// Whether `node` reads, assigns or sends `name` (`name` or `name:`).
fn names(node: &Node<'_>, name: &str) -> bool {
    match node {
        Node::Expression(expression) => match &expression.kind {
            ExpressionKind::Identifier(identifier) => identifier.name == name,
            ExpressionKind::Assign { target, .. } => target.name == name,
            _ => false,
        },
        Node::Message(message) => {
            is_setter_of(message, name)
                || matches!(&message.selector, Selector::Unary(selector) if selector == name)
        }
    }
}

/// Whether `name` appears anywhere in `statement`.
pub(super) fn mentions(statement: &Statement, name: &str) -> bool {
    let mut found = false;
    each_node_in_statement(statement, &mut |node| found |= names(&node, name));
    found
}

/// Whether `name` appears anywhere in `expression`.
pub(super) fn expression_mentions(expression: &Expression, name: &str) -> bool {
    let mut found = false;
    each_node(expression, &mut |node| found |= names(&node, name));
    found
}

/// The names `slots`' initializers and `statements` assign with `name::`,
/// their closures included: the only way a local is written.
pub(super) fn assigned_in<'tree>(
    slots: &'tree [SlotDeclaration],
    statements: &'tree [Statement],
) -> HashSet<&'tree str> {
    let mut assigned = HashSet::new();
    let mut visit = |node: Node<'tree>| {
        if let Node::Expression(expression) = node
            && let ExpressionKind::Assign { target, .. } = &expression.kind
        {
            assigned.insert(target.name.as_str());
        }
    };
    each_node_in_slots(slots, &mut visit);
    for statement in statements {
        each_node_in_statement(statement, &mut visit);
    }
    assigned
}
