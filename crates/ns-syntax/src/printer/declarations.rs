//! Declarations lowered to documents: the module, its classes, their
//! slots, members and methods, message patterns and type annotations, each
//! with the comments attached to it.

use std::rc::Rc;

use super::comments::{
    after, before, before_line_break, leading_on_its_line, on_its_line, OwnTokens,
};
use super::document::{concat, group, hard_line, indent, line, text, Document};
use super::expressions::{as_written, expression, message, statement};
use crate::ast::{
    AccessModifier, AttachedComment, ClassBody, ClassDeclaration, CodeBody, Member, MessagePattern,
    MethodDeclaration, Module, Parameter, SlotDeclaration, SlotMutability, SuperclassClause,
    SuperclassPrefix, TypeAnnotation, TypeToken,
};
use crate::positions::holds_a_comment;
use crate::token::TokenKind;

/// One indentation level.
const LEVEL: u32 = 2;
/// A continuation line: a broken class header, method pattern or `<:` chain.
const CONTINUATION: u32 = 4;

/// The prologue as written, each part on its own line, then the root class.
pub(super) fn module(module: &Module) -> Document {
    let mut tokens = OwnTokens::new(&module.comments);
    let mut parts = Vec::new();
    let mut prologue = false;
    if let Some(language_id) = &module.language_id {
        parts.push(tokens.next());
        parts.push(text(&language_id.name));
        prologue = true;
    }
    if let Some(category) = &module.category {
        parts.push(tokens.next());
        if prologue {
            parts.push(hard_line());
        }
        parts.push(as_written(&category.text));
        prologue = true;
    }
    parts.push(tokens.end());
    let class = &module.root_class;
    let line_break = if prologue { hard_line() } else { text("") };
    parts.push(on_its_line(
        &class.comments,
        line_break,
        class_declaration(class),
    ));
    concat(parts)
}

/// A class without the comments leading and trailing it, which the side or
/// module it stands in writes around its line.
fn class_declaration(class: &ClassDeclaration) -> Document {
    let mut tokens = OwnTokens::new(&class.comments);
    let mut parts = vec![class_header(class, &mut tokens)];
    let instance_side = &class.instance_side;
    if instance_side.delimiters.is_none() {
        // A mixin application ended by `.` has no body at all.
        parts.push(text("."));
        return concat(parts);
    }
    parts.push(text(" "));
    parts.push(before(&instance_side.comments));
    parts.push(instance_side_body(instance_side));
    parts.push(after(&instance_side.comments));
    tokens.child();
    // An empty class side carries nothing and is dropped.
    if let Some(class_side) = class
        .class_side
        .as_ref()
        .filter(|class_side| !class_side.members.is_empty() || !class_side.comments.is_empty())
    {
        parts.push(tokens.next());
        parts.push(text(" : "));
        parts.push(before(&class_side.comments));
        let mut side_tokens = OwnTokens::new(&class_side.comments);
        // The side's `(`.
        side_tokens.next();
        parts.push(members(&class_side.members, &mut side_tokens));
        parts.push(after(&class_side.comments));
        tokens.child();
    }
    parts.push(tokens.end());
    concat(parts)
}

/// `[access] class Name [type parameters] [factory] = [superclass] [<: …]`,
/// broken outside in: in the first of three layouts whose first line fits
/// -- all on one line; broken before `=`; every factory keyword on its own
/// line as well -- and, once broken before `=`, the `<:` chain on the `=`
/// line while that fits, else before each `<:`.
fn class_header(class: &ClassDeclaration, tokens: &mut OwnTokens) -> Document {
    let mut lead = Vec::new();
    if let Some(access) = class.access {
        lead.push(tokens.next());
        lead.push(text(access_word(access)));
    }
    lead.push(tokens.next());
    lead.push(text(if class.access.is_some() {
        " class"
    } else {
        "class"
    }));
    lead.push(tokens.next());
    lead.push(text(format!(" {}", class.name.name)));
    if let Some(type_parameters) = &class.type_parameters {
        lead.push(annotation(
            type_parameters,
            tokens,
            name_separator(type_parameters),
        ));
    }
    let factory = match &class.factory {
        Some(pattern) => {
            let (before_first, parts) = pattern_parts(pattern, tokens, true);
            lead.push(before_first);
            parts
        }
        None => Vec::new(),
    };
    let mut tail = Vec::new();
    if let Some(return_type) = &class.factory_return_type {
        tail.push(tokens.next());
        tail.push(text(" ^"));
        tail.push(annotation(return_type, tokens, ""));
    }
    if let Some(where_clause) = &class.factory_where_clause {
        tail.push(annotation(where_clause, tokens, " "));
    }
    tail.push(tokens.next());
    let lead = Rc::new(concat(lead));
    let factory: Vec<Rc<Document>> = factory.into_iter().map(Rc::new).collect();
    let tail = Rc::new(concat(tail));
    let equals_superclass = Rc::new(equals_superclass(class, tokens));
    let shared = |document: &Rc<Document>| Document::Shared(Rc::clone(document));

    let mut pattern_line = vec![shared(&lead)];
    for part in &factory {
        pattern_line.push(text(" "));
        pattern_line.push(shared(part));
    }
    pattern_line.push(shared(&tail));
    let pattern_line = Rc::new(concat(pattern_line));

    let one_line = concat(vec![
        shared(&pattern_line),
        text(" "),
        shared(&equals_superclass),
    ]);
    let broken_before_equals = concat(vec![
        shared(&pattern_line),
        indent(
            CONTINUATION,
            concat(vec![hard_line(), shared(&equals_superclass)]),
        ),
    ]);
    let mut alternatives = vec![one_line, broken_before_equals];
    if matches!(class.factory, Some(MessagePattern::Keyword { .. })) {
        let mut keywords = Vec::new();
        for part in &factory {
            keywords.push(hard_line());
            keywords.push(shared(part));
        }
        keywords.push(shared(&tail));
        keywords.push(hard_line());
        keywords.push(shared(&equals_superclass));
        alternatives.push(concat(vec![
            shared(&lead),
            indent(CONTINUATION, concat(keywords)),
        ]));
    }
    Document::Choice(alternatives)
}

/// `= [superclass] [<: mixin …]`, the `=` counted already. The `<:` chain is
/// the outer level: it stays on the `=` line when all of it fits there, else
/// every `<:` starts a line of its own, and only then is a clause's message
/// that is still too long for its line broken.
fn equals_superclass(class: &ClassDeclaration, tokens: &mut OwnTokens) -> Document {
    let mut parts = vec![text("=")];
    if let Some(superclass) = &class.superclass {
        parts.push(text(" "));
        parts.push(clause(superclass));
        tokens.child();
    }
    if class.mixin_applications.is_empty() {
        return concat(parts);
    }
    let mut links = Vec::new();
    for mixin in &class.mixin_applications {
        let comments = tokens.next();
        let written = Rc::new(concat(vec![text("<: "), clause(mixin)]));
        links.push((Rc::new(comments), written));
        tokens.child();
    }
    let shared = |document: &Rc<Document>| Document::Shared(Rc::clone(document));
    let mut on_the_line = Vec::new();
    let mut broken = Vec::new();
    for (comments, written) in &links {
        on_the_line.extend([shared(comments), line(), shared(written)]);
        broken.extend([shared(comments), hard_line(), shared(written)]);
    }
    let head = Rc::new(concat(parts));
    // Measured flat, the first alternative is the whole `=` line; a comment
    // that forces a break still breaks its group, which then reads as the
    // second.
    Document::Choice(vec![
        concat(vec![shared(&head), group(concat(on_the_line))]),
        concat(vec![shared(&head), concat(broken)]),
    ])
}

/// A superclass or mixin clause with its comments.
fn clause(clause: &SuperclassClause) -> Document {
    concat(vec![
        before(&clause.comments),
        superclass_clause(clause),
        after(&clause.comments),
    ])
}

/// `[self | super | outer Name] Receiver [message]`.
fn superclass_clause(clause: &SuperclassClause) -> Document {
    let mut tokens = OwnTokens::new(&clause.comments);
    let mut parts = Vec::new();
    let mut separator = "";
    match &clause.prefix {
        Some(SuperclassPrefix::SelfPrefix) => {
            parts.push(tokens.next());
            parts.push(text("self"));
            separator = " ";
        }
        Some(SuperclassPrefix::SuperPrefix) => {
            parts.push(tokens.next());
            parts.push(text("super"));
            separator = " ";
        }
        Some(SuperclassPrefix::Outer(name)) => {
            parts.push(tokens.next());
            parts.push(text("outer"));
            parts.push(tokens.next());
            parts.push(text(format!(" {}", name.name)));
            separator = " ";
        }
        None => {}
    }
    parts.push(tokens.next());
    parts.push(text(format!("{separator}{}", clause.receiver.name)));
    if let Some(type_arguments) = &clause.receiver_type_arguments {
        parts.push(annotation(
            type_arguments,
            &mut tokens,
            name_separator(type_arguments),
        ));
    }
    for sent in &clause.messages {
        parts.push(text(" "));
        parts.push(message(sent));
        tokens.child();
    }
    parts.push(tokens.end());
    concat(parts)
}

/// The header's `( … )` and the members' `( … )` of an instance side: the
/// slots, then the initializer expressions, one per line; `()` when it has
/// neither.
fn instance_side_body(side: &ClassBody) -> Document {
    let mut tokens = OwnTokens::new(&side.comments);
    tokens.on_own_lines(true);
    let mut inner = Vec::new();
    // The header's `(`.
    tokens.next();
    let bars_kept =
        !side.header_slots.is_empty() || holds_a_comment(side.slot_delimiters, &side.comments);
    if bars_kept {
        let bar = slot_bar(&side.header_slots);
        inner.push(tokens.next());
        inner.push(hard_line());
        inner.push(text(bar));
        let mut slots = Vec::new();
        for each in &side.header_slots {
            slots.push(on_its_line(
                &each.comments,
                hard_line(),
                concat(vec![slot(each), text(".")]),
            ));
            tokens.child();
        }
        slots.push(tokens.next());
        inner.push(indent(LEVEL, concat(slots)));
        inner.push(hard_line());
        inner.push(text(bar));
    }
    for each in &side.initializer_expressions {
        inner.push(on_its_line(
            &each.comments,
            hard_line(),
            concat(vec![statement(each), text(".")]),
        ));
        tokens.child();
    }
    inner.push(tokens.next());
    let header = opened_up(inner);
    tokens.on_own_lines(false);
    // Between the header's `)` and the members' `(`.
    let between = tokens.next();
    concat(vec![
        header,
        between,
        text(" "),
        members(&side.members, &mut tokens),
    ])
}

/// One side's members in `( … )`, a blank line between each two; `()` when
/// it has none. `tokens` have counted the side's `(`.
fn members(members: &[Member], tokens: &mut OwnTokens) -> Document {
    tokens.on_own_lines(true);
    let mut inner = Vec::new();
    for (index, each) in members.iter().enumerate() {
        let line_break = if index > 0 {
            concat(vec![hard_line(), hard_line()])
        } else {
            hard_line()
        };
        inner.push(on_its_line(member_comments(each), line_break, member(each)));
        tokens.child();
    }
    inner.push(tokens.next());
    tokens.on_own_lines(false);
    opened_up(inner)
}

/// `inner` between `(` and `)`, one level in, or `()` when it is empty.
fn opened_up(inner: Vec<Document>) -> Document {
    if inner.iter().all(is_empty) {
        return text("()");
    }
    concat(vec![
        text("("),
        indent(LEVEL, concat(inner)),
        hard_line(),
        text(")"),
    ])
}

/// Whether `document` writes nothing at all.
fn is_empty(document: &Document) -> bool {
    let mut pending = vec![document];
    while let Some(document) = pending.pop() {
        match document {
            Document::Concat(parts) => pending.extend(parts),
            Document::Text(content) if content.is_empty() => {}
            _ => return false,
        }
    }
    true
}

fn member_comments(member: &Member) -> &[AttachedComment] {
    match member {
        Member::Class(class) => &class.comments,
        Member::LazySlot(lazy_slot) => &lazy_slot.comments,
        Member::Method(method) => &method.comments,
    }
}

/// A member without the comments leading and trailing it.
fn member(member: &Member) -> Document {
    match member {
        Member::Class(class) => class_declaration(class),
        Member::LazySlot(lazy_slot) => concat(vec![slot(lazy_slot), text(".")]),
        Member::Method(method) => method_declaration(method),
    }
}

/// `[access] pattern [^<Type>] = (` on one line when it fits; otherwise
/// every keyword after the first on its own continuation line.
fn method_declaration(method: &MethodDeclaration) -> Document {
    let mut tokens = OwnTokens::new(&method.comments);
    let mut first = Vec::new();
    if let Some(access) = method.access {
        first.push(tokens.next());
        first.push(text(access_word(access)));
        first.push(text(" "));
    }
    let (before_first, parts) = pattern_parts(&method.pattern, &mut tokens, false);
    // The comments before the pattern follow the access modifier, before
    // the space.
    if method.access.is_some() {
        first.insert(first.len() - 1, before_first);
    } else {
        first.push(before_first);
    }
    let mut rest = Vec::new();
    for (index, part) in parts.into_iter().enumerate() {
        if index == 0 {
            first.push(part);
        } else {
            rest.push(line());
            rest.push(part);
        }
    }
    let mut tail = Vec::new();
    if let Some(return_type) = &method.return_type {
        tail.push(tokens.next());
        tail.push(text(" ^"));
        tail.push(annotation(return_type, &mut tokens, ""));
    }
    if let Some(where_clause) = &method.where_clause {
        tail.push(annotation(where_clause, &mut tokens, " "));
    }
    tail.push(tokens.next());
    tail.push(text(" ="));
    tail.push(tokens.next());
    tail.push(text(" "));
    concat(vec![
        group(concat(vec![
            concat(first),
            indent(CONTINUATION, concat(rest)),
            concat(tail),
        ])),
        method_body(&method.body, &mut tokens),
    ])
}

/// A method's `( … )`, always opened up: the temporaries between their
/// bars, then one statement per line, each with its period; `()` when it
/// holds nothing. `tokens` are the method's, its `(` counted.
fn method_body(body: &CodeBody, tokens: &mut OwnTokens) -> Document {
    let lines = code_body(body, false);
    let mut inner = vec![lines.ending_the_line_before, lines.lines];
    if !lines.empty {
        tokens.child();
    }
    tokens.on_own_lines(true);
    inner.push(tokens.next());
    tokens.on_own_lines(false);
    opened_up(inner)
}

/// A code body laid out one line per element.
pub(super) struct BodyLines {
    /// The comments leading the body that ended the line before it.
    pub(super) ending_the_line_before: Document,
    /// The temporaries and statements, every one after a line break but
    /// the first of a block's, and the comments among and after them.
    pub(super) lines: Document,
    /// The body has no token.
    pub(super) empty: bool,
}

/// `body`'s lines: in a method, each statement after a line break and with
/// its period; in a block, the first written where the block puts it and
/// the last with a period only when the block breaks (rule 6).
pub(super) fn code_body(body: &CodeBody, in_block: bool) -> BodyLines {
    let mut tokens = OwnTokens::new(&body.comments);
    tokens.on_own_lines(true);
    let bars_kept = !body.temporaries.is_empty()
        || holds_a_comment(body.temporaries_delimiters, &body.comments);
    let empty = !bars_kept && body.statements.is_empty();
    let mut lines = Vec::new();
    // A block puts its first line where it opens; a method's starts after
    // a line break, the body's leading comments after that.
    if !in_block && !empty {
        lines.push(hard_line());
    }
    lines.push(leading_on_its_line(&body.comments));
    let mut written = false;
    if bars_kept {
        let bar = slot_bar(&body.temporaries);
        lines.push(tokens.next());
        lines.push(text(bar));
        let mut temporaries = Vec::new();
        for each in &body.temporaries {
            temporaries.push(on_its_line(&each.comments, hard_line(), temporary(each)));
            tokens.child();
        }
        temporaries.push(tokens.next());
        lines.push(indent(LEVEL, concat(temporaries)));
        lines.push(hard_line());
        lines.push(text(bar));
        written = true;
    }
    let count = body.statements.len();
    for (index, each) in body.statements.iter().enumerate() {
        let period = if in_block && index + 1 == count {
            Document::IfBreak {
                broken: ".".to_string(),
                flat: String::new(),
            }
        } else {
            text(".")
        };
        let line_break = if written { hard_line() } else { text("") };
        lines.push(on_its_line(
            &each.comments,
            line_break,
            concat(vec![statement(each), period]),
        ));
        tokens.child();
        written = true;
    }
    lines.push(tokens.end());
    lines.push(after(&body.comments));
    BodyLines {
        ending_the_line_before: before_line_break(&body.comments),
        lines: concat(lines),
        empty,
    }
}

/// The parts of a pattern a broken header puts on lines of their own: each
/// keyword with its parameter, or the whole of a unary or binary pattern;
/// and before them, the comments that precede the pattern's first token.
/// The comments before a later part's first token close the part before;
/// `first_part_indented`: the layout puts the first part at the
/// continuation indent too (a factory broken at every keyword).
fn pattern_parts(
    pattern: &MessagePattern,
    tokens: &mut OwnTokens,
    first_part_indented: bool,
) -> (Document, Vec<Document>) {
    let before_first = tokens.next();
    let parts = match pattern {
        MessagePattern::Unary {
            selector,
            type_parameters,
        } => {
            let mut part = vec![text(&selector.name)];
            if let Some(type_parameters) = type_parameters {
                part.push(annotation(
                    type_parameters,
                    tokens,
                    name_separator(type_parameters),
                ));
            }
            vec![concat(part)]
        }
        MessagePattern::Binary {
            selector,
            parameter: argument,
        } => {
            let mut part = vec![text(&selector.name)];
            part.push(parameter(argument, tokens, " "));
            vec![concat(part)]
        }
        MessagePattern::Keyword { parts } => {
            let mut written: Vec<Vec<Document>> = Vec::new();
            for (index, part) in parts.iter().enumerate() {
                if index > 0 {
                    let mut before_keyword = tokens.next();
                    // A comment on a line of its own there stands at the
                    // continuation indent, as the keywords after the first do.
                    if index == 1 && !first_part_indented {
                        before_keyword = indent(CONTINUATION, before_keyword);
                    }
                    if let Some(previous) = written.last_mut() {
                        previous.push(before_keyword);
                    }
                }
                written.push(vec![
                    text(format!("{}:", part.keyword.name)),
                    parameter(&part.parameter, tokens, " "),
                ]);
            }
            written.into_iter().map(concat).collect()
        }
    };
    (before_first, parts)
}

/// A parameter after `separator`, its name and its type counted in
/// `tokens`.
pub(super) fn parameter(
    parameter: &Parameter,
    tokens: &mut OwnTokens,
    separator: &str,
) -> Document {
    let mut parts = vec![
        tokens.next(),
        text(format!("{separator}{}", parameter.name.name)),
    ];
    if let Some(type_annotation) = &parameter.type_annotation {
        parts.push(annotation(
            type_annotation,
            tokens,
            name_separator(type_annotation),
        ));
    }
    concat(parts)
}

/// A slot without its period: `[lazy] [access] name [<Type>] [= value]`.
pub(super) fn slot(slot: &SlotDeclaration) -> Document {
    let mut tokens = OwnTokens::new(&slot.comments);
    let mut words = Vec::new();
    match (slot.lazy, slot.access_before_lazy) {
        (true, true) => {
            words.extend(slot.access.map(access_word));
            words.push("lazy");
        }
        (true, false) => {
            words.push("lazy");
            words.extend(slot.access.map(access_word));
        }
        (false, _) => words.extend(slot.access.map(access_word)),
    }
    words.push(&slot.name.name);
    let mut parts = Vec::new();
    for (index, word) in words.iter().enumerate() {
        parts.push(tokens.next());
        parts.push(text(if index == 0 {
            (*word).to_string()
        } else {
            format!(" {word}")
        }));
    }
    if let Some(type_annotation) = &slot.type_annotation {
        parts.push(annotation(
            type_annotation,
            &mut tokens,
            name_separator(type_annotation),
        ));
    }
    let assignment = match slot.mutability {
        SlotMutability::Immutable => Some(" ="),
        SlotMutability::MutableInitialized => Some(" ::="),
        SlotMutability::MutableBare => None,
    };
    if let (Some(assignment), Some(initializer)) = (assignment, &slot.initializer) {
        parts.push(tokens.next());
        parts.push(text(format!("{assignment} ")));
        parts.push(expression(initializer));
        tokens.child();
    }
    parts.push(tokens.end());
    concat(parts)
}

/// A temporary: a bare one has no period, an initialized one needs it.
pub(super) fn temporary(temporary: &SlotDeclaration) -> Document {
    if temporary.initializer.is_some() {
        concat(vec![slot(temporary), text(".")])
    } else {
        slot(temporary)
    }
}

/// The bar of a slot list: `||` for simultaneous slots, else `|`.
pub(super) fn slot_bar(slots: &[SlotDeclaration]) -> &'static str {
    if slots.iter().any(|each| each.simultaneous) {
        "||"
    } else {
        "|"
    }
}

fn access_word(access: AccessModifier) -> &'static str {
    match access {
        AccessModifier::Public => "public",
        AccessModifier::Protected => "protected",
        AccessModifier::Private => "private",
    }
}

/// What separates an annotation from the name it follows: one space before
/// an opening `<`, none before a `[`.
pub(super) fn name_separator(annotation: &TypeAnnotation) -> &'static str {
    match annotation.tokens.first() {
        Some(first) if first.kind == TokenKind::LessThan => " ",
        _ => "",
    }
}

/// An annotation after `separator`, its tokens spaced as `type_text` spaces
/// them and counted in `tokens`, with the comments between them.
pub(super) fn annotation(
    annotation: &TypeAnnotation,
    tokens: &mut OwnTokens,
    separator: &str,
) -> Document {
    let mut parts = Vec::new();
    let all = &annotation.tokens;
    for (index, token) in all.iter().enumerate() {
        let space = match index.checked_sub(1) {
            None => separator,
            Some(previous) => {
                let before_previous = previous.checked_sub(1).map(|before| &all[before]);
                if spaced(before_previous, &all[previous], token) {
                    " "
                } else {
                    ""
                }
            }
        };
        parts.push(match token.kind {
            TokenKind::LessThan
            | TokenKind::LeftBracket
            | TokenKind::LeftBrace
            | TokenKind::LeftParenthesis => tokens.next_opener(),
            TokenKind::GreaterThan
            | TokenKind::RightBracket
            | TokenKind::RightBrace
            | TokenKind::RightParenthesis => tokens.next_closer(),
            _ => tokens.next(),
        });
        parts.push(text(format!("{space}{}", token.text)));
    }
    concat(parts)
}

/// Whether one space separates `previous` from `next` inside a type. The
/// annotation keeps no whitespace, so the spacing is decided here, from
/// the tokens alone: rule 20's spaces, the spacing expressions have (after
/// a keyword, around a binary operator, a `where` clause's `::` and its
/// braces), and a space wherever the two written together would lex as
/// something else, so a reprinted type always reads back the same. The
/// token before `previous` tells the second `:` of `::` from a block
/// type's `:`.
fn spaced(before_previous: Option<&TypeToken>, previous: &TypeToken, next: &TypeToken) -> bool {
    use TokenKind::{
        Bar, BinarySelector, Colon, DoubleBar, Identifier, Keyword, LeftBrace, LeftBracket,
        LeftParenthesis, LessThan, MixinOperator, RightBrace, RightBracket,
    };
    let is_comma = |token: &TypeToken| token.kind == BinarySelector && token.text == ",";
    let is_double_colon = before_previous.is_some_and(|before| before.kind == Colon);
    let wanted = if is_comma(next) || (previous.kind == Colon && next.kind == Colon) {
        false
    } else if is_comma(previous) || (previous.kind == Colon && is_double_colon) {
        true
    } else {
        match (previous.kind, next.kind) {
            (Bar | DoubleBar | MixinOperator | BinarySelector | Keyword | LeftBrace, _)
            | (_, Bar | DoubleBar | MixinOperator | BinarySelector | LeftBrace | RightBrace) => {
                true
            }
            (Identifier | RightBracket, LessThan) | (Identifier, LeftParenthesis) => true,
            (LeftBracket | LeftParenthesis, Colon) => false,
            (_, Colon) => true,
            _ => false,
        }
    };
    wanted || !joins_cleanly(previous, next)
}

/// Whether `previous` and `next` written without a space lex back as the
/// same two tokens.
fn joins_cleanly(previous: &TypeToken, next: &TypeToken) -> bool {
    let joined = format!("{}{}", previous.text, next.text);
    let (tokens, diagnostics) = crate::lexer::Lexer::new(&joined).run();
    if !diagnostics.is_empty() {
        return false;
    }
    match tokens.as_slice() {
        [first, second, end] => {
            first.kind == previous.kind
                && second.kind == next.kind
                && end.kind == TokenKind::EndOfInput
                && first.span.text(&joined) == previous.text
        }
        _ => false,
    }
}
