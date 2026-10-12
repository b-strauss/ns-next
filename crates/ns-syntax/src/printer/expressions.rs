//! Expressions and statements lowered to documents: chains of messages that
//! break outside in, keyword messages that hug their block arguments,
//! cascades, blocks, tuples, parentheses and literals.

use std::rc::Rc;

use super::declarations::{after_name, parameter, slot_bar, temporary, type_text};
use super::document::{concat, group, hard_line, indent, line, soft_line, text, Document};
use crate::ast::{
    BlockExpression, Cascade, Expression, ExpressionKind, Literal, MessageSend, Selector, Send,
    Statement, StatementKind,
};

/// One indentation level: the body of a block or a tuple.
const LEVEL: u32 = 2;
/// A continuation line of a broken message.
const CONTINUATION: u32 = 4;

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
        // Never broken after `::`: a long value breaks inside itself.
        ExpressionKind::Assign { target, value, .. } => concat(vec![
            text(format!("{}:: ", target.name)),
            self::expression(value),
        ]),
        ExpressionKind::Block(block) => match block_parts(block) {
            Some(parts) => block_frame(&parts, false),
            None => text(empty_block(block)),
        },
        ExpressionKind::DynamicArray { elements, .. } => tuple(elements),
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

/// Whether anything stands around `expression`'s shape.
fn has_envelope(expression: &Expression) -> bool {
    !expression.parentheses.is_empty() || expression.type_arguments.is_some()
}

/// The precedence level of a run of messages that breaks as one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Unary,
    Binary,
}

/// The messages of one level sent to the same receiver, each after a
/// `Line`: the level breaks as a whole, every link on its own line.
struct Run {
    level: Level,
    links: Vec<Document>,
}

/// A chain of messages is as deep as the source is long, so it is walked
/// link by link from the receiver it starts at, never by recursing into
/// each receiver. Consecutive unary or binary links form one run, grouped
/// so the outer level (the run around it, the keyword message sent to it)
/// breaks first.
fn send_chain(send: &Send) -> Document {
    let (links, start) = send.receiver_chain();
    let mut receiver = (!start.synthesized).then(|| receiver_head(expression(start)));
    let mut run: Option<Run> = None;
    for index in (0..links.len()).rev() {
        let message = &links[index].message;
        match &message.selector {
            Selector::Unary(_) | Selector::Binary(_) => {
                let level = if matches!(message.selector, Selector::Unary(_)) {
                    Level::Unary
                } else {
                    Level::Binary
                };
                if run.as_ref().is_some_and(|current| current.level != level) {
                    receiver = finish_run(receiver, run.take());
                }
                let sent = self::message(message);
                match (&mut run, &receiver) {
                    (Some(current), _) => current.links.extend([line(), sent]),
                    // An implicit receiver prints nothing: the first
                    // selector heads the run.
                    (None, None) => {
                        receiver = Some(sent);
                        run = Some(Run {
                            level,
                            links: Vec::new(),
                        });
                    }
                    (None, Some(_)) => {
                        run = Some(Run {
                            level,
                            links: vec![line(), sent],
                        });
                    }
                }
            }
            Selector::Keyword(_) => {
                receiver = finish_run(receiver, run.take());
                receiver = Some(keyword_message(receiver, message));
            }
        }
        // The link outside this one holds this link's envelope as its
        // receiver; the outermost link's envelope is the caller's.
        if let Some(outer) = index.checked_sub(1) {
            let envelope = &links[outer].receiver;
            if has_envelope(envelope) {
                let completed = finish_run(receiver, run.take()).unwrap_or_else(|| text(""));
                receiver = Some(receiver_head(enveloped(envelope, completed)));
            }
        }
    }
    finish_run(receiver, run.take()).unwrap_or_else(|| text(""))
}

/// `receiver` with the messages of `run` sent to it, the continuation
/// lines one step in.
fn finish_run(receiver: Option<Document>, run: Option<Run>) -> Option<Document> {
    let Some(run) = run else {
        return receiver;
    };
    let mut parts = Vec::new();
    parts.extend(receiver);
    if !run.links.is_empty() {
        parts.push(indent(CONTINUATION, concat(run.links)));
    }
    Some(group(concat(parts)))
}

/// A receiver that breaks by itself (a multi-line block, a multi-line
/// string) opens at the line end and its messages follow its last line, as
/// rule 15 asks of a receiver block: its break is kept from forcing the
/// messages sent to it onto lines of their own.
fn receiver_head(receiver: Document) -> Document {
    if forces_break(&receiver) {
        Document::Choice(vec![receiver])
    } else {
        receiver
    }
}

/// `receiver msg1; msg2; …`, always broken: the receiver alone on the first
/// line, each message on its own continuation line.
fn cascaded(cascade: &Cascade) -> Document {
    let mut messages = std::iter::once(&cascade.first)
        .chain(&cascade.rest)
        .map(message);
    let mut parts = Vec::new();
    // An implicit receiver prints nothing: the first message takes the
    // first line.
    let mut continued = Vec::new();
    if cascade.receiver.synthesized {
        parts.extend(messages.next());
    } else {
        parts.push(expression(&cascade.receiver));
        continued.push(hard_line());
        continued.extend(messages.next());
    }
    for each in messages {
        continued.push(text(";"));
        continued.push(hard_line());
        continued.push(each);
    }
    parts.push(indent(CONTINUATION, concat(continued)));
    concat(parts)
}

/// A message without the receiver it is sent to.
pub(super) fn message(message: &MessageSend) -> Document {
    let eventual = eventual_prefix(message);
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
        Selector::Keyword(_) => keyword_message(None, message),
    }
}

fn eventual_prefix(message: &MessageSend) -> &'static str {
    if message.eventual {
        "<-: "
    } else {
        ""
    }
}

/// An argument of a keyword message as the alternatives of its layout use
/// it: a block that can open at the line end, in parts, or anything else,
/// whole. Each is lowered once and shared by every alternative.
enum Argument {
    Block(BlockParts),
    Other(Rc<Document>),
}

/// `receiver kw1: arg1 kw2: arg2 …`. Without a block argument that can
/// break, one group: the first keyword on the receiver's line, every
/// further one on its own continuation line when it breaks. With one, the
/// first of these whose first line fits: the whole message on one line
/// (left out when a part of it breaks by itself, since a group that must
/// break would "fit" on its first line); hugged, the keyword parts joined
/// by spaces and every block argument opened at the line end, the blocks
/// of one message breaking together; the group broken, each block on one
/// line where it fits.
fn keyword_message(receiver: Option<Document>, message: &MessageSend) -> Document {
    let Selector::Keyword(selector) = &message.selector else {
        return self::message(message);
    };
    let keywords: Vec<&str> = selector.split_terminator(':').collect();
    let arguments: Vec<Argument> = message
        .arguments
        .iter()
        .map(|argument| match huggable_block(argument) {
            Some(parts) => Argument::Block(parts),
            None => Argument::Other(Rc::new(expression(argument))),
        })
        .collect();
    let eventual = eventual_prefix(message);
    if !arguments
        .iter()
        .any(|argument| matches!(argument, Argument::Block(_)))
    {
        let receiver = receiver.map(Rc::new);
        return standard(&receiver, eventual, &keywords, &arguments, false);
    }
    let receiver = receiver.map(Rc::new);
    let mut alternatives = Vec::new();
    let one_line = standard(&receiver, eventual, &keywords, &arguments, false);
    if !forces_break(&one_line) {
        alternatives.push(one_line);
    }
    alternatives.push(hugged(&receiver, eventual, &keywords, &arguments));
    alternatives.push(standard(&receiver, eventual, &keywords, &arguments, true));
    Document::Choice(alternatives)
}

/// The keyword parts after the first on continuation lines: a group that
/// breaks only when it does not fit, or, `broken`, the form already broken.
fn standard(
    receiver: &Option<Rc<Document>>,
    eventual: &str,
    keywords: &[&str],
    arguments: &[Argument],
    broken: bool,
) -> Document {
    let mut head = Vec::new();
    if let Some(receiver) = receiver {
        head.push(Document::Shared(Rc::clone(receiver)));
        head.push(text(" "));
    }
    let mut rest = Vec::new();
    for (index, (keyword, argument)) in keywords.iter().zip(arguments).enumerate() {
        let argument = match argument {
            Argument::Block(parts) => block_frame(parts, false),
            Argument::Other(document) => Document::Shared(Rc::clone(document)),
        };
        if index == 0 {
            head.push(text(format!("{eventual}{keyword}: ")));
            head.push(argument);
        } else {
            rest.push(if broken { hard_line() } else { line() });
            rest.push(text(format!("{keyword}: ")));
            rest.push(argument);
        }
    }
    if !rest.is_empty() {
        head.push(indent(CONTINUATION, concat(rest)));
    }
    let message = concat(head);
    if broken {
        message
    } else {
        group(message)
    }
}

/// Every block argument opened at the line end and broken, the keyword
/// parts joined by spaces. Another argument that can break is a group of
/// its own, so it is measured again on the `]` line; one that cannot is
/// left bare, since a group around it would use up the measuring again that
/// something breakable after it needs.
fn hugged(
    receiver: &Option<Rc<Document>>,
    eventual: &str,
    keywords: &[&str],
    arguments: &[Argument],
) -> Document {
    let mut parts = Vec::new();
    if let Some(receiver) = receiver {
        parts.push(Document::Shared(Rc::clone(receiver)));
        parts.push(text(" "));
    }
    for (index, (keyword, argument)) in keywords.iter().zip(arguments).enumerate() {
        if index == 0 {
            parts.push(text(format!("{eventual}{keyword}: ")));
        } else {
            parts.push(text(format!(" {keyword}: ")));
        }
        parts.push(match argument {
            Argument::Block(block) => block_frame(block, true),
            Argument::Other(document) if is_text(document) => Document::Shared(Rc::clone(document)),
            Argument::Other(document) => group(Document::Shared(Rc::clone(document))),
        });
    }
    concat(parts)
}

/// What a block is made of apart from its brackets: the `[` line and the
/// body, shared by every frame it is laid out in.
struct BlockParts {
    /// `[`, or `[:each |` with parameters.
    opening: String,
    has_parameters: bool,
    body: Rc<Document>,
    /// The body cannot stand on one line: it holds temporaries, two
    /// statements, or a statement that breaks by itself.
    multi_line: bool,
}

/// `argument`'s parts when it is a block written as a bare argument, with
/// a body to open at the line end; a parenthesised block stays whole.
fn huggable_block(argument: &Expression) -> Option<BlockParts> {
    match &argument.kind {
        ExpressionKind::Block(block) if !has_envelope(argument) => block_parts(block),
        _ => None,
    }
}

/// `[]` or `[:each |]`: a block with nothing inside.
fn empty_block(block: &BlockExpression) -> String {
    if block.parameters.is_empty() {
        "[]".to_string()
    } else {
        format!("{}]", block_opening(block))
    }
}

fn block_opening(block: &BlockExpression) -> String {
    if block.parameters.is_empty() {
        return "[".to_string();
    }
    let parameters: Vec<String> = block
        .parameters
        .iter()
        .map(|each| format!(":{}", parameter(each)))
        .collect();
    format!("[{} |", parameters.join(" "))
}

/// The parts of a block with a body; `None` for an empty one.
fn block_parts(block: &BlockExpression) -> Option<BlockParts> {
    let body = &block.body;
    if body.temporaries.is_empty() && body.statements.is_empty() {
        return None;
    }
    let mut inner = Vec::new();
    if !body.temporaries.is_empty() {
        let bar = slot_bar(&body.temporaries);
        let mut temporaries = Vec::new();
        for each in &body.temporaries {
            temporaries.push(hard_line());
            temporaries.push(temporary(each));
        }
        inner.push(text(bar));
        inner.push(indent(LEVEL, concat(temporaries)));
        inner.push(hard_line());
        inner.push(text(bar));
        if !body.statements.is_empty() {
            inner.push(hard_line());
        }
    }
    inner.extend(sequence(body.statements.iter().map(statement), hard_line));
    let body = concat(inner);
    Some(BlockParts {
        opening: block_opening(block),
        has_parameters: !block.parameters.is_empty(),
        multi_line: forces_break(&body),
        body: Rc::new(body),
    })
}

/// A block on one line when it fits and holds one statement and no
/// temporaries, else `[` (with the parameters) at the line end, the body one
/// level in, `]` back at the line's indentation. `forced` breaks it whatever
/// the width, as a hugged block does. A statement that breaks by itself
/// behind a `Choice` (a message hugging its own blocks) does not break the
/// group around it, so the frame is told.
fn block_frame(parts: &BlockParts, forced: bool) -> Document {
    let mut frame = vec![text(&parts.opening)];
    if forced || parts.multi_line {
        frame.push(Document::BreakParent);
    }
    // With parameters, a one-line block reads `[:each | body]`.
    let opening_break = if parts.has_parameters {
        line()
    } else {
        soft_line()
    };
    frame.push(indent(
        LEVEL,
        concat(vec![
            opening_break,
            Document::Shared(Rc::clone(&parts.body)),
        ]),
    ));
    frame.push(soft_line());
    frame.push(text("]"));
    group(concat(frame))
}

/// The elements of a sequence, each but the last followed by `.` and a
/// `separator`, the last by a period only when its group breaks (rule 6):
/// a one-line block or tuple has none after its last element.
fn sequence(
    elements: impl ExactSizeIterator<Item = Document>,
    separator: fn() -> Document,
) -> Vec<Document> {
    let count = elements.len();
    let mut parts = Vec::new();
    for (index, element) in elements.enumerate() {
        parts.push(element);
        if index + 1 < count {
            parts.push(text("."));
            parts.push(separator());
        } else {
            parts.push(Document::IfBreak {
                broken: ".".to_string(),
                flat: String::new(),
            });
        }
    }
    parts
}

/// `{a. b. c}` on one line when it fits, else `{` at the line end, one
/// element per line one level in, `}` back at the line's indentation.
fn tuple(elements: &[Expression]) -> Document {
    if elements.is_empty() {
        return text("{}");
    }
    let mut inner = vec![soft_line()];
    inner.extend(sequence(elements.iter().map(expression), line));
    let inner = concat(inner);
    // An element that breaks by itself behind a `Choice` leaves the tuple
    // no one-line form.
    let mut frame = vec![text("{")];
    if forces_break(&inner) {
        frame.push(Document::BreakParent);
    }
    frame.extend([indent(LEVEL, inner), soft_line(), text("}")]);
    group(concat(frame))
}

/// Whether `document` is text alone, with no place to break.
fn is_text(document: &Document) -> bool {
    let mut pending = vec![document];
    while let Some(document) = pending.pop() {
        match document {
            Document::Text(_) | Document::Verbatim(_) | Document::IfBreak { .. } => {}
            Document::Shared(content) => pending.push(content),
            Document::Concat(parts) => pending.extend(parts),
            _ => return false,
        }
    }
    true
}

/// Whether `document` holds a line break whatever the width, outside any
/// `Choice` but its first alternative: what the layout writes when it lays
/// the document out flat.
fn forces_break(document: &Document) -> bool {
    let mut pending = vec![document];
    while let Some(document) = pending.pop() {
        match document {
            Document::HardLine | Document::LiteralLine | Document::BreakParent => return true,
            Document::Verbatim(content) if content.contains('\n') => return true,
            Document::Indent(_, content) | Document::Group(content) => pending.push(content),
            Document::Shared(content) => pending.push(content),
            Document::Concat(parts) => pending.extend(parts),
            Document::Choice(alternatives) => pending.extend(alternatives.first()),
            Document::Text(_)
            | Document::Verbatim(_)
            | Document::Line
            | Document::SoftLine
            | Document::IfBreak { .. }
            | Document::LineSuffix(_) => {}
        }
    }
    false
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
