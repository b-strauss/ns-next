//! Declarations lowered to documents: the module, its classes, their
//! slots, members and methods, message patterns and type annotations.

use super::document::{concat, group, hard_line, indent, line, text, Document};
use super::expressions::{as_written, expression, message, statement};
use crate::ast::{
    AccessModifier, ClassBody, ClassDeclaration, CodeBody, Member, MessagePattern,
    MethodDeclaration, Module, Parameter, SlotDeclaration, SlotMutability, SuperclassClause,
    SuperclassPrefix, TypeAnnotation, TypeToken,
};
use crate::token::TokenKind;

/// One indentation level.
const LEVEL: u32 = 2;
/// A continuation line: a broken class header, method pattern or `<:` chain.
const CONTINUATION: u32 = 4;

/// The prologue as written, each part on its own line, then the root class.
pub(super) fn module(module: &Module) -> Document {
    let mut parts = Vec::new();
    if let Some(language_id) = &module.language_id {
        parts.push(text(&language_id.name));
        parts.push(hard_line());
    }
    if let Some(category) = &module.category {
        parts.push(as_written(&category.text));
        parts.push(hard_line());
    }
    parts.push(class_declaration(&module.root_class));
    concat(parts)
}

fn class_declaration(class: &ClassDeclaration) -> Document {
    let mut parts = vec![class_header(class)];
    let instance_side = &class.instance_side;
    if instance_side.delimiters.is_none() {
        // A mixin application ended by `.` has no body at all.
        parts.push(text("."));
        return concat(parts);
    }
    parts.push(text(" "));
    parts.push(class_header_body(instance_side));
    parts.push(text(" "));
    parts.push(side(&instance_side.members));
    // An empty class side carries nothing and is dropped.
    if let Some(class_side) = class
        .class_side
        .as_ref()
        .filter(|class_side| !class_side.members.is_empty())
    {
        parts.push(text(" : "));
        parts.push(side(&class_side.members));
    }
    concat(parts)
}

/// `[access] class Name [type parameters] [factory] = [superclass] [<: …]`,
/// broken outside in: in the first of three layouts whose first line fits
/// -- all on one line; broken before `=`; every factory keyword on its own
/// line as well -- and, once broken before `=`, the `<:` chain on the `=`
/// line while that fits, else before each `<:`.
fn class_header(class: &ClassDeclaration) -> Document {
    let mut lead = format!("{}class {}", access(class.access), class.name.name);
    if let Some(type_parameters) = &class.type_parameters {
        lead.push_str(&after_name(type_parameters));
    }
    let factory = class
        .factory
        .as_ref()
        .map(pattern_parts)
        .unwrap_or_default();
    let mut tail = String::new();
    if let Some(return_type) = &class.factory_return_type {
        tail.push_str(&format!(" ^{}", type_text(return_type)));
    }
    if let Some(where_clause) = &class.factory_where_clause {
        tail.push_str(&format!(" {}", type_text(where_clause)));
    }
    let equals_superclass = || {
        let mut parts = vec![text("=")];
        if let Some(superclass) = &class.superclass {
            parts.push(text(" "));
            parts.push(superclass_clause(superclass));
        }
        if !class.mixin_applications.is_empty() {
            let mut chain = Vec::new();
            for clause in &class.mixin_applications {
                chain.push(line());
                chain.push(text("<: "));
                chain.push(superclass_clause(clause));
            }
            parts.push(group(concat(chain)));
        }
        concat(parts)
    };
    let mut pattern_line = lead.clone();
    for part in &factory {
        pattern_line.push(' ');
        pattern_line.push_str(part);
    }
    pattern_line.push_str(&tail);

    let one_line = concat(vec![
        text(pattern_line.clone()),
        text(" "),
        equals_superclass(),
    ]);
    let broken_before_equals = concat(vec![
        text(pattern_line),
        indent(CONTINUATION, concat(vec![hard_line(), equals_superclass()])),
    ]);
    let mut alternatives = vec![one_line, broken_before_equals];
    if matches!(class.factory, Some(MessagePattern::Keyword { .. })) {
        let mut keywords = Vec::new();
        for part in &factory {
            keywords.push(hard_line());
            keywords.push(text(part));
        }
        keywords.push(text(tail));
        keywords.push(hard_line());
        keywords.push(equals_superclass());
        alternatives.push(concat(vec![
            text(lead),
            indent(CONTINUATION, concat(keywords)),
        ]));
    }
    Document::Choice(alternatives)
}

/// `[self | super | outer Name] Receiver [message]`.
fn superclass_clause(clause: &SuperclassClause) -> Document {
    let mut head = match &clause.prefix {
        Some(SuperclassPrefix::SelfPrefix) => "self ".to_string(),
        Some(SuperclassPrefix::SuperPrefix) => "super ".to_string(),
        Some(SuperclassPrefix::Outer(name)) => format!("outer {} ", name.name),
        None => String::new(),
    };
    head.push_str(&clause.receiver.name);
    if let Some(type_arguments) = &clause.receiver_type_arguments {
        head.push_str(&after_name(type_arguments));
    }
    let mut parts = vec![text(head)];
    for sent in &clause.messages {
        parts.push(text(" "));
        parts.push(message(sent));
    }
    concat(parts)
}

/// The header's `( … )`: the slots, then the initializer expressions, one
/// per line; `()` when it has neither.
fn class_header_body(side: &ClassBody) -> Document {
    let mut inner = Vec::new();
    if !side.header_slots.is_empty() {
        let bar = slot_bar(&side.header_slots);
        let mut slots = Vec::new();
        for each in &side.header_slots {
            slots.push(hard_line());
            slots.push(slot(each));
            slots.push(text("."));
        }
        inner.push(hard_line());
        inner.push(text(bar));
        inner.push(indent(LEVEL, concat(slots)));
        inner.push(hard_line());
        inner.push(text(bar));
    }
    for each in &side.initializer_expressions {
        inner.push(hard_line());
        inner.push(statement(each));
        inner.push(text("."));
    }
    opened_up(inner)
}

/// One side's members in `( … )`, a blank line between each two; `()` when
/// it has none.
fn side(members: &[Member]) -> Document {
    let mut inner = Vec::new();
    for (index, each) in members.iter().enumerate() {
        if index > 0 {
            inner.push(hard_line());
        }
        inner.push(hard_line());
        inner.push(member(each));
    }
    opened_up(inner)
}

/// `inner` between `(` and `)`, one level in, or `()` when it is empty.
fn opened_up(inner: Vec<Document>) -> Document {
    if inner.is_empty() {
        return text("()");
    }
    concat(vec![
        text("("),
        indent(LEVEL, concat(inner)),
        hard_line(),
        text(")"),
    ])
}

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
    let parts = pattern_parts(&method.pattern);
    let mut first = access(method.access).to_string();
    let mut rest = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index == 0 {
            first.push_str(part);
        } else {
            rest.push(line());
            rest.push(text(part));
        }
    }
    let mut tail = String::new();
    if let Some(return_type) = &method.return_type {
        tail.push_str(&format!(" ^{}", type_text(return_type)));
    }
    if let Some(where_clause) = &method.where_clause {
        tail.push_str(&format!(" {}", type_text(where_clause)));
    }
    tail.push_str(" = ");
    concat(vec![
        group(concat(vec![
            text(first),
            indent(CONTINUATION, concat(rest)),
            text(tail),
        ])),
        code_body(&method.body),
    ])
}

/// A method's `( … )`, always opened up: the temporaries between their
/// bars, then one statement per line, each with its period; `()` when it
/// holds nothing.
fn code_body(body: &CodeBody) -> Document {
    let mut inner = Vec::new();
    if !body.temporaries.is_empty() {
        let bar = slot_bar(&body.temporaries);
        let mut temporaries = Vec::new();
        for each in &body.temporaries {
            temporaries.push(hard_line());
            temporaries.push(temporary(each));
        }
        inner.push(hard_line());
        inner.push(text(bar));
        inner.push(indent(LEVEL, concat(temporaries)));
        inner.push(hard_line());
        inner.push(text(bar));
    }
    for each in &body.statements {
        inner.push(hard_line());
        inner.push(statement(each));
        inner.push(text("."));
    }
    opened_up(inner)
}

/// The parts of a pattern a broken header puts on lines of their own: each
/// keyword with its parameter, or the whole of a unary or binary pattern.
fn pattern_parts(pattern: &MessagePattern) -> Vec<String> {
    match pattern {
        MessagePattern::Unary {
            selector,
            type_parameters,
        } => {
            let mut written = selector.name.clone();
            if let Some(type_parameters) = type_parameters {
                written.push_str(&after_name(type_parameters));
            }
            vec![written]
        }
        MessagePattern::Binary {
            selector,
            parameter: argument,
        } => vec![format!("{} {}", selector.name, parameter(argument))],
        MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|part| format!("{}: {}", part.keyword.name, parameter(&part.parameter)))
            .collect(),
    }
}

pub(super) fn parameter(parameter: &Parameter) -> String {
    let mut written = parameter.name.name.clone();
    if let Some(type_annotation) = &parameter.type_annotation {
        written.push_str(&after_name(type_annotation));
    }
    written
}

/// A slot without its period: `[lazy] [access] name [<Type>] [= value]`.
pub(super) fn slot(slot: &SlotDeclaration) -> Document {
    let head_modifiers = match (slot.lazy, slot.access_before_lazy) {
        (true, true) => format!("{}lazy ", access(slot.access)),
        (true, false) => format!("lazy {}", access(slot.access)),
        (false, _) => access(slot.access).to_string(),
    };
    let mut head = head_modifiers;
    head.push_str(&slot.name.name);
    if let Some(type_annotation) = &slot.type_annotation {
        head.push_str(&after_name(type_annotation));
    }
    let assignment = match slot.mutability {
        SlotMutability::Immutable => " = ",
        SlotMutability::MutableInitialized => " ::= ",
        SlotMutability::MutableBare => "",
    };
    match &slot.initializer {
        Some(initializer) => concat(vec![
            text(format!("{head}{assignment}")),
            expression(initializer),
        ]),
        None => text(head),
    }
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

fn access(access: Option<AccessModifier>) -> &'static str {
    match access {
        Some(AccessModifier::Public) => "public ",
        Some(AccessModifier::Protected) => "protected ",
        Some(AccessModifier::Private) => "private ",
        None => "",
    }
}

/// An annotation as it follows a name: one space before an opening `<`,
/// none before a `[`.
pub(super) fn after_name(annotation: &TypeAnnotation) -> String {
    let written = type_text(annotation);
    match annotation.tokens.first() {
        Some(first) if first.kind == TokenKind::LessThan => format!(" {written}"),
        _ => written,
    }
}

/// The annotation's tokens with the spaces rule 20 wants and no others.
pub(super) fn type_text(annotation: &TypeAnnotation) -> String {
    let mut written = String::new();
    for (index, token) in annotation.tokens.iter().enumerate() {
        if let Some(previous) = index.checked_sub(1) {
            let before_previous = previous.checked_sub(1);
            let tokens = &annotation.tokens;
            if spaced(
                before_previous.map(|before| &tokens[before]),
                &tokens[previous],
                token,
            ) {
                written.push(' ');
            }
        }
        written.push_str(&token.text);
    }
    written
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
