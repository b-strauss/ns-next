//! Expressions and statements lowered to documents: chains of messages that
//! break outside in, keyword messages that hug their block arguments,
//! cascades, blocks, tuples, parentheses and literals, each with the
//! comments attached to it.

use std::rc::Rc;

use super::comments::{after, before, on_its_line, OwnTokens};
use super::declarations::{annotation, code_body, name_separator, parameter};
use super::document::{concat, group, hard_line, indent, line, soft_line, text, Document};
use crate::ast::{
    BlockExpression, Cascade, Expression, ExpressionKind, Literal, MessageSend, Selector, Send,
    Statement, StatementKind,
};

/// One indentation level: the body of a block or a tuple.
const LEVEL: u32 = 2;
/// A continuation line of a broken message.
const CONTINUATION: u32 = 4;

/// A statement without the comments leading and trailing it, which the
/// sequence it stands in writes around its line.
pub(super) fn statement(statement: &Statement) -> Document {
    let mut tokens = OwnTokens::new(&statement.comments);
    let mut parts = Vec::new();
    match &statement.kind {
        StatementKind::Expression(value) => parts.push(expression(value)),
        StatementKind::Return(value) => {
            parts.push(tokens.next());
            parts.push(text("^"));
            parts.push(expression(value));
        }
    }
    tokens.child();
    parts.push(tokens.end());
    concat(parts)
}

/// An expression with the comments leading and trailing it. A leading
/// comment that ends its line, or stands on one of its own, puts the
/// expression on the next line: it continues the line it follows (after a
/// keyword, an operator, `::`, `=` or `^`), so it goes at +4, not at the
/// column a new statement would take.
pub(super) fn expression(expression: &Expression) -> Document {
    let written = concat(vec![
        before(&expression.comments),
        expression_alone(expression),
        after(&expression.comments),
    ]);
    let breaks_before = expression.comments.iter().any(|comment| {
        comment.anchor == crate::ast::Anchor::Before
            && comment.placement != crate::ast::Placement::Inline
    });
    if breaks_before {
        indent(CONTINUATION, written)
    } else {
        written
    }
}

/// An expression without the comments leading and trailing it.
fn expression_alone(expression: &Expression) -> Document {
    if expression.synthesized {
        return text("");
    }
    enveloped(expression, |tokens| match &expression.kind {
        ExpressionKind::Identifier(identifier) => {
            concat(vec![tokens.next(), text(&identifier.name)])
        }
        ExpressionKind::Literal(value) => {
            concat(vec![tokens.skip(literal_tokens(value)), literal(value)])
        }
        ExpressionKind::Send(send) => {
            let chain = send_chain(send);
            tokens.child();
            chain
        }
        ExpressionKind::Cascade(cascade) => cascaded(cascade, tokens),
        // Never broken after `::`: a long value breaks inside itself.
        ExpressionKind::Assign { target, value, .. } => {
            let assigned = concat(vec![
                tokens.next(),
                text(format!("{}:: ", target.name)),
                self::expression(value),
            ]);
            tokens.child();
            assigned
        }
        ExpressionKind::Block(block) => {
            let written = concat(vec![
                before(&block.comments),
                block_alone(block),
                after(&block.comments),
            ]);
            tokens.child();
            written
        }
        ExpressionKind::DynamicArray { elements, .. } => tuple(elements, tokens),
    })
}

/// The own tokens a literal is written in: a pattern literal's are as many
/// as the lexer finds in its text.
fn literal_tokens(literal: &Literal) -> u32 {
    match literal {
        Literal::Pattern { text: written, .. } => {
            let (tokens, _) = crate::lexer::Lexer::new(written).run();
            u32::try_from(tokens.len().saturating_sub(1)).unwrap_or(u32::MAX)
        }
        _ => 1,
    }
}

/// The shape `write` lowers, with what surrounds it in `expression`: each
/// parenthesis's `(`, outermost first; the shape; its type arguments; then
/// each `)`, innermost first, with its own type arguments -- the own tokens
/// in the order `attach.rs` counts them, the comments between them.
fn enveloped(expression: &Expression, write: impl FnOnce(&mut OwnTokens) -> Document) -> Document {
    let mut tokens = OwnTokens::new(&expression.comments);
    let mut parts = Vec::new();
    for _ in &expression.parentheses {
        parts.push(tokens.next_opener());
        parts.push(text("("));
    }
    parts.push(write(&mut tokens));
    if let Some(arguments) = &expression.type_arguments {
        parts.push(annotation(
            arguments,
            &mut tokens,
            name_separator(arguments),
        ));
    }
    for parenthesis in &expression.parentheses {
        parts.push(tokens.next_closer());
        parts.push(text(")"));
        if let Some(arguments) = &parenthesis.type_arguments {
            parts.push(annotation(arguments, &mut tokens, ""));
        }
    }
    parts.push(tokens.end());
    concat(parts)
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

/// The messages of one level sent to the same receiver: the level breaks
/// as a whole, every link on its own line.
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
    let mut receiver = (!start.synthesized).then(|| {
        let written = expression(start);
        // A receiver block breaks when it does not fit, whatever follows
        // it: its messages are decided on its `]` line (rule 15).
        if matches!(start.kind, ExpressionKind::Block(_)) {
            Document::Choice(vec![written])
        } else {
            receiver_head(written)
        }
    });
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
                    (Some(current), _) => current.links.push(sent),
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
                            links: vec![sent],
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
            let only_trailing = envelope
                .comments
                .iter()
                .all(|comment| comment.anchor == crate::ast::Anchor::After);
            if has_envelope(envelope) || !only_trailing {
                let completed = finish_run(receiver, run.take()).unwrap_or_else(|| text(""));
                let wrapped = enveloped(envelope, |tokens| {
                    tokens.child();
                    completed
                });
                receiver = Some(receiver_head(concat(vec![
                    before(&envelope.comments),
                    wrapped,
                    after(&envelope.comments),
                ])));
            } else if !envelope.comments.is_empty() {
                // A comment trailing a link stays after it, and the run
                // goes on: the level it stands in breaks around it.
                let trailing = after(&envelope.comments);
                match run.as_mut().and_then(|current| current.links.last_mut()) {
                    Some(link) => {
                        let written = std::mem::replace(link, text(""));
                        *link = concat(vec![written, trailing]);
                    }
                    None => {
                        let written = receiver.take().unwrap_or_else(|| text(""));
                        receiver = Some(concat(vec![written, trailing]));
                    }
                }
            }
        }
    }
    finish_run(receiver, run.take()).unwrap_or_else(|| text(""))
}

/// `receiver` with the messages of `run` sent to it, the continuation
/// lines one step in. A receiver that breaks by itself ends on a line of its
/// own (`]`), so the messages after it are a group of their own, decided on
/// that line. A link that breaks by itself (an operand hugging its blocks)
/// hugs as a block argument does: the links up to it stay on the line,
/// outside any continuation indent, so its body is indented from the line
/// it opens on, and the links after it are again a group of their own.
fn finish_run(receiver: Option<Document>, run: Option<Run>) -> Option<Document> {
    let Some(run) = run else {
        return receiver;
    };
    if run.links.is_empty() {
        return receiver;
    }
    let Some(last_breaking) = run.links.iter().rposition(forces_break) else {
        let receiver_breaks = receiver.as_ref().is_some_and(breaks_by_itself);
        let mut parts = Vec::new();
        parts.extend(receiver);
        return Some(if receiver_breaks {
            parts.push(group(continued(run.links)));
            concat(parts)
        } else {
            parts.push(continued(run.links));
            group(concat(parts))
        });
    };
    let receiver = receiver.map(Rc::new);
    let links: Vec<Rc<Document>> = run.links.into_iter().map(Rc::new).collect();
    let shared = |link: &Rc<Document>| Document::Shared(Rc::clone(link));
    let mut hugged = Vec::new();
    hugged.extend(receiver.as_ref().map(shared));
    for link in &links[..=last_breaking] {
        hugged.push(text(" "));
        hugged.push(shared(link));
    }
    let after = &links[last_breaking + 1..];
    if !after.is_empty() {
        hugged.push(group(continued(after.iter().map(shared).collect())));
    }
    let mut broken = Vec::new();
    broken.extend(receiver.as_ref().map(shared));
    let mut lines = Vec::new();
    for link in &links {
        lines.push(hard_line());
        lines.push(shared(link));
    }
    broken.push(indent(CONTINUATION, concat(lines)));
    Some(Document::Choice(vec![concat(hugged), concat(broken)]))
}

/// `links`, each on a continuation line of its own when broken.
fn continued(links: Vec<Document>) -> Document {
    let mut lines = Vec::new();
    for link in links {
        lines.push(line());
        lines.push(link);
    }
    indent(CONTINUATION, concat(lines))
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
/// line, each message on its own continuation line. `tokens` are the
/// cascade expression's, its `;`s among them.
fn cascaded(cascade: &Cascade, tokens: &mut OwnTokens) -> Document {
    let mut parts = Vec::new();
    let mut continued = Vec::new();
    let implicit = cascade.receiver.synthesized;
    // An implicit receiver prints nothing: the first message takes the
    // first line.
    if implicit {
        parts.push(message(&cascade.first));
    } else {
        parts.push(expression(&cascade.receiver));
        continued.push(hard_line());
        continued.push(message(&cascade.first));
    }
    tokens.child();
    for each in &cascade.rest {
        continued.push(tokens.next());
        continued.push(text(";"));
        continued.push(hard_line());
        continued.push(message(each));
        tokens.child();
    }
    parts.push(indent(CONTINUATION, concat(continued)));
    concat(parts)
}

/// A message without the receiver it is sent to, with its comments.
pub(super) fn message(message: &MessageSend) -> Document {
    if matches!(message.selector, Selector::Keyword(_)) {
        return keyword_message(None, message);
    }
    let mut tokens = OwnTokens::new(&message.comments);
    let mut parts = vec![before(&message.comments)];
    parts.extend(eventual_prefix(message, &mut tokens));
    let space = if message.eventual { " " } else { "" };
    match &message.selector {
        Selector::Unary(selector) => {
            parts.push(tokens.next());
            parts.push(text(format!("{space}{selector}")));
            if let Some(arguments) = &message.type_arguments {
                parts.push(annotation(
                    arguments,
                    &mut tokens,
                    name_separator(arguments),
                ));
            }
        }
        Selector::Binary(selector) | Selector::Keyword(selector) => {
            parts.push(tokens.next());
            parts.push(text(format!("{space}{selector} ")));
            parts.extend(message.arguments.iter().map(expression));
            tokens.child();
        }
    }
    parts.push(tokens.end());
    parts.push(after(&message.comments));
    concat(parts)
}

/// `<-:` and the comments after it, for an eventual send; the space after
/// it is the next token's.
fn eventual_prefix(message: &MessageSend, tokens: &mut OwnTokens) -> Vec<Document> {
    if message.eventual {
        vec![tokens.next(), text("<-:")]
    } else {
        Vec::new()
    }
}

/// An argument of a keyword message as the alternatives of its layout use
/// it: a block that can open at the line end, in parts; anything else
/// whole, with whether it breaks by itself (a message hugging its own
/// blocks), which then hugs as a block does. Each is lowered once and
/// shared by every alternative.
enum Argument {
    Block(BlockParts),
    Other {
        document: Rc<Document>,
        breaks: bool,
    },
}

impl Argument {
    fn hugs(&self) -> bool {
        match self {
            Argument::Block(_) => true,
            Argument::Other { breaks, .. } => *breaks,
        }
    }
}

/// What stands before the first keyword.
enum Head {
    /// Nothing: the receiver is implicit, or the message is a cascade's.
    Nothing,
    Receiver(Rc<Document>),
    /// A receiver that broke by itself, written before the message, which
    /// follows on its last line after a space.
    AfterReceiver,
}

impl Head {
    fn lead(&self, parts: &mut Vec<Document>) {
        match self {
            Head::Nothing => {}
            Head::Receiver(receiver) => {
                parts.push(Document::Shared(Rc::clone(receiver)));
                parts.push(text(" "));
            }
            Head::AfterReceiver => parts.push(text(" ")),
        }
    }
}

/// The parts of a keyword message every alternative of its layout writes
/// the same: the keywords, the comments leading the message and the
/// eventual operator, and the comments due before each further keyword
/// (after the argument before it) and after the last argument.
struct KeywordParts {
    keywords: Vec<String>,
    lead: Rc<Document>,
    /// `between[index]` follows argument `index`.
    between: Vec<Rc<Document>>,
}

impl KeywordParts {
    fn new(message: &MessageSend, selector: &str) -> KeywordParts {
        let mut tokens = OwnTokens::new(&message.comments);
        let mut lead = vec![before(&message.comments)];
        lead.extend(eventual_prefix(message, &mut tokens));
        let space = if message.eventual { " " } else { "" };
        lead.push(tokens.next());
        lead.push(text(space));
        let keywords: Vec<String> = selector
            .split_terminator(':')
            .map(|keyword| format!("{keyword}: "))
            .collect();
        let mut between = Vec::new();
        for index in 0..keywords.len() {
            tokens.child();
            let due = if index + 1 < keywords.len() {
                tokens.next()
            } else {
                concat(vec![tokens.end(), after(&message.comments)])
            };
            between.push(Rc::new(due));
        }
        KeywordParts {
            keywords,
            lead: Rc::new(concat(lead)),
            between,
        }
    }

    fn between(&self, index: usize) -> Document {
        Document::Shared(Rc::clone(&self.between[index]))
    }
}

/// `receiver kw1: arg1 kw2: arg2 …`. Without an argument that hugs, one
/// group: the first keyword on the receiver's line, every further one on
/// its own continuation line when it breaks. With one, the first of these
/// whose first line fits: the whole message on one line (left out when a
/// part of it breaks by itself, since a group that must break would "fit"
/// on its first line); hugged, the keyword parts joined by spaces outside
/// any continuation indent and every block argument opened at the line
/// end, the blocks of one message breaking together; the group broken,
/// each block on one line where it fits. A receiver that breaks by itself
/// comes first, and the message is decided on its last line.
fn keyword_message(receiver: Option<Document>, message: &MessageSend) -> Document {
    let Selector::Keyword(selector) = &message.selector else {
        return self::message(message);
    };
    let parts = KeywordParts::new(message, selector);
    let arguments: Vec<Argument> = message
        .arguments
        .iter()
        .map(|argument| match huggable_block(argument) {
            Some(parts) => Argument::Block(parts),
            None => {
                let document = expression(argument);
                Argument::Other {
                    breaks: forces_break(&document),
                    document: Rc::new(document),
                }
            }
        })
        .collect();
    let (written_first, head) = match receiver {
        Some(receiver) if breaks_by_itself(&receiver) => (Some(receiver), Head::AfterReceiver),
        Some(receiver) => (None, Head::Receiver(Rc::new(receiver))),
        None => (None, Head::Nothing),
    };
    let sent = if arguments.iter().any(Argument::hugs) {
        let mut alternatives = Vec::new();
        let one_line = standard(&head, &parts, &arguments, false);
        if !forces_break(&one_line) {
            alternatives.push(one_line);
        }
        alternatives.push(hugged(&head, &parts, &arguments));
        alternatives.push(standard(&head, &parts, &arguments, true));
        Document::Choice(alternatives)
    } else {
        standard(&head, &parts, &arguments, false)
    };
    match written_first {
        Some(receiver) => concat(vec![receiver, sent]),
        None => sent,
    }
}

/// The keyword parts after the first on continuation lines: a group that
/// breaks only when it does not fit, or, `broken`, the form already broken.
fn standard(head: &Head, parts: &KeywordParts, arguments: &[Argument], broken: bool) -> Document {
    let mut first = Vec::new();
    head.lead(&mut first);
    first.push(Document::Shared(Rc::clone(&parts.lead)));
    let mut rest = Vec::new();
    for (index, (keyword, argument)) in parts.keywords.iter().zip(arguments).enumerate() {
        let argument = match argument {
            Argument::Block(block) => block_frame(block, false),
            Argument::Other { document, .. } => Document::Shared(Rc::clone(document)),
        };
        if index == 0 {
            first.push(text(keyword));
            first.push(argument);
            first.push(parts.between(index));
        } else {
            rest.push(if broken { hard_line() } else { line() });
            rest.push(text(keyword));
            rest.push(argument);
            rest.push(parts.between(index));
        }
    }
    if !rest.is_empty() {
        first.push(indent(CONTINUATION, concat(rest)));
    }
    let message = concat(first);
    if broken {
        message
    } else {
        group(message)
    }
}

/// Every block argument opened at the line end and broken, the keyword
/// parts joined by spaces. Another argument that can break is a group of
/// its own, so it is measured again on the `]` line; one that cannot, or
/// one that breaks by itself anyway, is left bare, since a group around it
/// would use up the measuring again that something breakable after it
/// needs.
fn hugged(head: &Head, parts: &KeywordParts, arguments: &[Argument]) -> Document {
    let mut written = Vec::new();
    head.lead(&mut written);
    written.push(Document::Shared(Rc::clone(&parts.lead)));
    for (index, (keyword, argument)) in parts.keywords.iter().zip(arguments).enumerate() {
        if index > 0 {
            written.push(text(" "));
        }
        written.push(text(keyword));
        written.push(match argument {
            Argument::Block(block) => block_frame(block, true),
            Argument::Other { document, breaks } if *breaks || is_text(document) => {
                Document::Shared(Rc::clone(document))
            }
            Argument::Other { document, .. } => group(Document::Shared(Rc::clone(document))),
        });
        written.push(parts.between(index));
    }
    concat(written)
}

/// What a block is made of apart from its frame: the `[` line and the
/// body, shared by every frame it is laid out in.
struct BlockParts {
    /// `[`, or `[:each |` with parameters, and the comments among them and
    /// those leading the body that end the line.
    opening: Rc<Document>,
    has_parameters: bool,
    body: Rc<Document>,
    /// The comments before `]`.
    closing: Rc<Document>,
    /// The body cannot stand on one line: it holds temporaries, two
    /// statements, or a statement that breaks by itself.
    multi_line: bool,
}

/// `argument`'s parts when it is a block written as a bare argument, with
/// a body to open at the line end; a parenthesised or commented block
/// stays whole.
fn huggable_block(argument: &Expression) -> Option<BlockParts> {
    match &argument.kind {
        ExpressionKind::Block(block)
            if !has_envelope(argument)
                && argument.comments.is_empty()
                && block.comments.iter().all(|comment| {
                    !matches!(
                        comment.anchor,
                        crate::ast::Anchor::Before | crate::ast::Anchor::After
                    )
                }) =>
        {
            block_parts(block)
        }
        _ => None,
    }
}

/// A block without the comments leading and trailing it.
fn block_alone(block: &BlockExpression) -> Document {
    match block_parts(block) {
        Some(parts) => block_frame(&parts, false),
        None => empty_block(block),
    }
}

/// `[]`, `[:each |]`, or such a block holding only comments.
fn empty_block(block: &BlockExpression) -> Document {
    let mut tokens = OwnTokens::new(&block.comments);
    let mut parts = opening(block, &mut tokens);
    let closing = tokens.next_closer();
    parts.push(indent(LEVEL, closing));
    parts.push(text("]"));
    concat(parts)
}

/// `[` and the parameters with their `|`, the comments among them.
fn opening(block: &BlockExpression, tokens: &mut OwnTokens) -> Vec<Document> {
    let mut parts = vec![tokens.next_opener(), text("[")];
    for (index, each) in block.parameters.iter().enumerate() {
        parts.push(tokens.next());
        parts.push(text(if index == 0 { ":" } else { " :" }));
        parts.push(parameter(each, tokens, ""));
    }
    if !block.parameters.is_empty() {
        parts.push(tokens.next());
        parts.push(text(" |"));
    }
    parts
}

/// The parts of a block with a body; `None` for one whose body has no
/// token.
fn block_parts(block: &BlockExpression) -> Option<BlockParts> {
    let lines = code_body(&block.body, true);
    if lines.empty {
        return None;
    }
    let mut tokens = OwnTokens::new(&block.comments);
    let mut opening = opening(block, &mut tokens);
    opening.push(lines.ending_the_line_before);
    tokens.child();
    let closing = tokens.next_closer();
    let multi_line = forces_break(&lines.lines) || forces_break(&closing);
    Some(BlockParts {
        opening: Rc::new(concat(opening)),
        has_parameters: !block.parameters.is_empty(),
        multi_line,
        body: Rc::new(lines.lines),
        closing: Rc::new(closing),
    })
}

/// A block on one line when it fits and holds one statement and no
/// temporaries, else `[` (with the parameters) at the line end, the body one
/// level in, `]` back at the line's indentation. `forced` breaks it whatever
/// the width, as a hugged block does. A statement that breaks by itself
/// behind a `Choice` (a message hugging its own blocks) does not break the
/// group around it, so the frame is told.
fn block_frame(parts: &BlockParts, forced: bool) -> Document {
    let mut frame = vec![Document::Shared(Rc::clone(&parts.opening))];
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
            Document::Shared(Rc::clone(&parts.closing)),
        ]),
    ));
    frame.push(soft_line());
    frame.push(text("]"));
    group(concat(frame))
}

/// `{a. b. c}` on one line when it fits, else `{` at the line end, one
/// element per line one level in, `}` back at the line's indentation.
/// `tokens` are the tuple expression's.
fn tuple(elements: &[Expression], tokens: &mut OwnTokens) -> Document {
    let mut frame = vec![tokens.next_opener(), text("{")];
    if elements.is_empty() {
        frame.push(indent(LEVEL, tokens.next_closer()));
        frame.push(text("}"));
        return concat(frame);
    }
    let count = elements.len();
    let mut inner = Vec::new();
    for (index, element) in elements.iter().enumerate() {
        let period = if index + 1 < count {
            text(".")
        } else {
            Document::IfBreak {
                broken: ".".to_string(),
                flat: String::new(),
            }
        };
        let line_break = if index == 0 { soft_line() } else { line() };
        inner.push(on_its_line(
            &element.comments,
            line_break,
            concat(vec![expression_alone(element), period]),
        ));
        tokens.child();
    }
    inner.push(tokens.next_closer());
    let inner = concat(inner);
    // An element that breaks by itself behind a `Choice` leaves the tuple
    // no one-line form.
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

/// Whether `document`, sent a message, may end on a line of its own: it
/// holds a line break whatever the width, or a message that hugs its blocks
/// when it does not fit. Its messages are then decided on its last line, as
/// rule 13 decides the keywords after a `]`; measured as part of one line
/// with them, it would count as written flat and push them onto continuation
/// lines while it hugs anyway.
fn breaks_by_itself(document: &Document) -> bool {
    if forces_break(document) {
        return true;
    }
    let mut pending = vec![document];
    while let Some(document) = pending.pop() {
        match document {
            Document::Choice(_) => return true,
            Document::Indent(_, content) | Document::Group(content) => pending.push(content),
            Document::Shared(content) => pending.push(content),
            Document::Concat(parts) => pending.extend(parts),
            _ => {}
        }
    }
    false
}

/// Whether `document` holds a line break whatever the width, outside any
/// `Choice` but its first alternative: what the layout writes when it lays
/// the document out flat.
fn forces_break(document: &Document) -> bool {
    let mut pending = vec![document];
    while let Some(document) = pending.pop() {
        match document {
            Document::HardLine | Document::BreakParent | Document::FreshLine => return true,
            Document::Verbatim(content) if content.contains(['\n', '\r']) => return true,
            Document::Shifted { lines, .. } if lines.len() > 1 => return true,
            Document::Indent(_, content) | Document::Group(content) => pending.push(content),
            Document::Shared(content) => pending.push(content),
            Document::Concat(parts) => pending.extend(parts),
            Document::Choice(alternatives) => pending.extend(alternatives.first()),
            Document::Text(_)
            | Document::Verbatim(_)
            | Document::Shifted { .. }
            | Document::Line
            | Document::SoftLine
            | Document::IfBreak { .. }
            | Document::LineSuffix(_)
            | Document::Trailing(_)
            | Document::LeadingStart
            | Document::LeadingEnd => {}
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
