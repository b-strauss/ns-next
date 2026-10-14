//! A document's hints as LSP diagnostics and code actions: the quick fix
//! of each hint the requested range touches, and beside them one action
//! that applies every hint of the file. The edits are plain `changes`:
//! with full synchronisation the server always holds the text the client
//! asks about.

use std::collections::HashMap;

use crate::hints::Hint;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Diagnostic, DiagnosticSeverity,
    NumberOrString, Position, Range, TextEdit, Uri, WorkspaceEdit,
};
use ns_syntax::Span;

use crate::documents::LineIndex;

fn range_of(span: Span, text: &str, line_index: &LineIndex) -> Range {
    Range::new(
        line_index.position(text, span.start),
        line_index.position(text, span.end),
    )
}

/// `hint` as the diagnostic the editor shows at the slot's name: a hint,
/// the weakest severity, which IntelliJ shows as a weak warning.
pub fn diagnostic(hint: &Hint, text: &str, line_index: &LineIndex) -> Diagnostic {
    Diagnostic {
        range: range_of(hint.span, text, line_index),
        severity: Some(DiagnosticSeverity::HINT),
        code: Some(NumberOrString::String(hint.code().to_string())),
        source: Some("ns-next".to_string()),
        message: hint.message(),
        ..Diagnostic::default()
    }
}

fn ordered(position: Position) -> (u32, u32) {
    (position.line, position.character)
}

fn touches(first: Range, second: Range) -> bool {
    ordered(first.start) <= ordered(second.end) && ordered(second.start) <= ordered(first.end)
}

fn workspace_edit(uri: &Uri, edits: Vec<TextEdit>) -> WorkspaceEdit {
    WorkspaceEdit {
        changes: Some(HashMap::from([(uri.clone(), edits)])),
        ..WorkspaceEdit::default()
    }
}

/// The quick fixes of the hints `range` touches, then the fix for the
/// whole file, which `whole_file` produces only when a hint is touched (and
/// which is left out when it answers `None`); nothing when `range` touches
/// no hint.
pub fn actions(
    uri: &Uri,
    hints: &[Hint],
    text: &str,
    line_index: &LineIndex,
    range: Range,
    whole_file: impl FnOnce() -> Option<(Span, String)>,
) -> Vec<CodeActionOrCommand> {
    let touched: Vec<&Hint> = hints
        .iter()
        .filter(|hint| touches(range_of(hint.span, text, line_index), range))
        .collect();
    if touched.is_empty() {
        return Vec::new();
    }
    let mut actions: Vec<CodeActionOrCommand> = touched
        .iter()
        .map(|hint| {
            let edits = hint
                .edits
                .iter()
                .map(|edit| TextEdit {
                    range: range_of(edit.span, text, line_index),
                    new_text: edit.text.clone(),
                })
                .collect();
            CodeActionOrCommand::CodeAction(CodeAction {
                title: hint.title(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diagnostic(hint, text, line_index)]),
                edit: Some(workspace_edit(uri, edits)),
                is_preferred: Some(true),
                ..CodeAction::default()
            })
        })
        .collect();
    let Some(changed) = whole_file() else {
        return actions;
    };
    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
        title: "Apply all hints in this file".to_string(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(
            touched
                .iter()
                .map(|hint| diagnostic(hint, text, line_index))
                .collect(),
        ),
        edit: Some(workspace_edit(
            uri,
            vec![TextEdit {
                range: range_of(changed.0, text, line_index),
                new_text: changed.1,
            }],
        )),
        ..CodeAction::default()
    }));
    actions
}

/// The one edit that applies every hint of `text`, or `None` when nothing
/// changes or applying panics. Later rounds of `apply_all` parse and edit
/// texts the analysis never saw, so a defect there is possible; like a panic
/// in the analysis (`analysis::guarded`) it must not take the server, and
/// the editor's support for every other file, down with it. The per-hint
/// quick fixes are then still offered.
pub fn whole_file_edit(text: &str) -> Option<(Span, String)> {
    let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::hints::apply_all(text)
    }))
    .ok()?;
    let changed = changed_span(text, &applied);
    (applied != text).then_some(changed)
}

/// The smallest span of `before` whose replacement gives `after`, and that
/// replacement: the two texts with their common prefix and suffix left
/// out, both cut on character boundaries.
fn changed_span(before: &str, after: &str) -> (Span, String) {
    let mut prefix = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(first, second)| first == second)
        .count();
    while !before.is_char_boundary(prefix) || !after.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let mut suffix = before[prefix..]
        .bytes()
        .rev()
        .zip(after[prefix..].bytes().rev())
        .take_while(|(first, second)| first == second)
        .count();
    while !before.is_char_boundary(before.len() - suffix)
        || !after.is_char_boundary(after.len() - suffix)
    {
        suffix -= 1;
    }
    (
        Span::new(prefix as u32, (before.len() - suffix) as u32),
        after[prefix..after.len() - suffix].to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::changed_span;
    use ns_syntax::Span;

    #[test]
    fn the_changed_span_leaves_out_the_common_prefix_and_suffix() {
        assert_eq!(
            changed_span("a count b", "a count ::= 1. b"),
            (Span::new(8, 8), "::= 1. ".to_string())
        );
        assert_eq!(
            changed_span("same", "same"),
            (Span::new(4, 4), String::new())
        );
        // `ü` and `ß` are two bytes each: the change is `ß` (bytes 4..6).
        assert_eq!(
            changed_span("Grüße", "Grüne"),
            (Span::new(4, 6), "n".to_string())
        );
    }
}
