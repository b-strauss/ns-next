//! The text edits a fix makes, and applying them. Every edit is minimal:
//! a fix changes the code its hint names and leaves the layout around it
//! as it was, so a file that was never formatted is not reformatted by a
//! quick fix.

use ns_syntax::ast::{Expression, SlotDeclaration, Statement};
use ns_syntax::token::TokenKind;
use ns_syntax::Span;

/// The text of `span` replaced by `text`; an empty span inserts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub span: Span,
    pub text: String,
}

/// `text` with `edits` applied. The edits must not overlap.
pub fn apply(text: &str, edits: &[Edit]) -> String {
    let mut ordered: Vec<&Edit> = edits.iter().collect();
    ordered.sort_by_key(|edit| (edit.span.start, edit.span.end));
    let mut result = String::with_capacity(text.len());
    let mut position = 0;
    for edit in ordered {
        let start = edit.span.start as usize;
        debug_assert!(start >= position, "overlapping edits");
        result.push_str(&text[position..start]);
        result.push_str(&edit.text);
        position = edit.span.end as usize;
    }
    result.push_str(&text[position..]);
    result
}

/// Whether two edits touch the same text or insert at the same place:
/// two such edits cannot be applied together.
pub(super) fn overlap(first: &Edit, second: &Edit) -> bool {
    (first.span.start < second.span.end && second.span.start < first.span.end)
        || first.span.start == second.span.start
}

/// Whether `text` holds the start of a comment. A fix never deletes or
/// moves one: a comment belongs to the code beside it. A string that
/// spells `///` or `(*` counts too, which costs a hint, never correctness.
pub(super) fn holds_a_comment(text: &str) -> bool {
    text.contains("///") || text.contains("(*")
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |index| index + 1)
}

/// The offset of the `\n` that ends the line holding `offset`, or the
/// text's end.
fn line_end(text: &str, offset: usize) -> usize {
    text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index)
}

fn column(text: &str, offset: usize) -> usize {
    offset - line_start(text, offset)
}

/// Where a slot's declaration ends: after its `.` when it has an
/// initializer, else after its name or its type. A bare slot's span runs
/// on to the token after it (`parser/declaration.rs`), so it cannot say.
pub(super) fn declaration_end(slot: &SlotDeclaration) -> u32 {
    if slot.initializer.is_some() {
        return slot.span.end;
    }
    slot.type_annotation
        .as_ref()
        .map_or(slot.name.span.end, |annotation| annotation.span.end)
}

/// The `.` a bare slot is written with, which the grammar makes optional
/// (§6.3.2). Lexed, not searched: a comment after the name may spell one.
pub(super) fn bare_period(text: &str, slot: &SlotDeclaration) -> Option<Span> {
    if slot.initializer.is_some() {
        return None;
    }
    let from = declaration_end(slot) as usize;
    let (tokens, _) = ns_syntax::lexer::Lexer::new(&text[from..slot.span.end as usize]).run();
    let period = tokens.iter().find(|token| token.kind == TokenKind::Dot)?;
    let start = from as u32 + period.span.start;
    Some(Span::new(start, start + 1))
}

/// Where a slot's text ends, its `.` included: what moving it takes along.
pub(super) fn written_end(text: &str, slot: &SlotDeclaration) -> u32 {
    bare_period(text, slot).map_or_else(|| declaration_end(slot), |period| period.end)
}

/// The text after `offset` up to the end of its line.
pub(super) fn rest_of_line(text: &str, offset: u32) -> &str {
    let offset = offset as usize;
    &text[offset..line_end(text, offset)]
}

/// ` ::= value.`, the value as written. Its continuation lines move right
/// by as many columns as the declaration stands right of the statement
/// (left when it stands left of it), so they keep their place relative to
/// the code they continue.
pub(super) fn initializer(
    text: &str,
    value: &Expression,
    statement_start: u32,
    declaration_start: u32,
) -> String {
    let declaration_column = column(text, declaration_start as usize);
    let statement_column = column(text, statement_start as usize);
    let written = &text[value.span.start as usize..value.span.end as usize];
    let mut lines = written.split('\n');
    let mut moved = lines.next().unwrap_or_default().to_string();
    for line in lines {
        moved.push('\n');
        if declaration_column >= statement_column {
            moved.push_str(&" ".repeat(declaration_column - statement_column));
            moved.push_str(line);
        } else {
            let blanks = line.len() - line.trim_start_matches(' ').len();
            let removed = blanks.min(statement_column - declaration_column);
            moved.push_str(&line[removed..]);
        }
    }
    format!(" ::= {moved}.")
}

/// The edit that deletes `statement` with its `.`: its whole line, line
/// break included, when nothing else stands on it, else the statement and
/// the blanks after it. `None` when a comment stands in the statement or
/// on the rest of its line: deleting the one or leaving the other behind
/// would change what the comment is about.
pub(super) fn delete_statement(text: &str, statement: &Statement) -> Option<Edit> {
    let start = statement.span.start as usize;
    let mut end = statement.span.end as usize;
    let after = text[end..].trim_start_matches([' ', '\t', '\r', '\n']);
    if after.starts_with('.') {
        end = text.len() - after.len() + 1;
    }
    let rest = &text[end..line_end(text, end)];
    if holds_a_comment(&text[start..end]) || holds_a_comment(rest) {
        return None;
    }
    let before = &text[line_start(text, start)..start];
    let span = if before.trim().is_empty() && rest.trim().is_empty() {
        let finish = (line_end(text, end) + 1).min(text.len());
        Span::new(line_start(text, start) as u32, finish as u32)
    } else {
        let blanks = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        Span::new(start as u32, (end + blanks) as u32)
    };
    Some(Edit {
        span,
        text: String::new(),
    })
}

/// The span of the `::=` of a slot declared with it.
pub(super) fn assignment_operator(text: &str, slot: &SlotDeclaration) -> Option<Span> {
    let initializer = slot.initializer.as_ref()?;
    let from = slot
        .type_annotation
        .as_ref()
        .map_or(slot.name.span.end, |annotation| annotation.span.end) as usize;
    // Lexed, not searched: a comment between the name and the initializer
    // may spell `::=` itself.
    let (tokens, _) =
        ns_syntax::lexer::Lexer::new(&text[from..initializer.span.start as usize]).run();
    let operator = tokens
        .iter()
        .find(|token| token.kind == TokenKind::ColonColonEquals)?;
    let start = from as u32 + operator.span.start;
    Some(Span::new(start, start + 3))
}
