//! Hint A: a bare local whose first value a statement assigns can take that
//! value in its declaration. A fix must keep the order of evaluation:
//! every initializer of the section first, then the moved value, as when
//! it was a statement. At the section's first statement that holds
//! whatever the value is; after other statements only for a value that
//! can neither fail nor see what they change (`settled`). Every fix stands
//! on its own: once the first statement has moved, the next one is first.

use ns_syntax::ast::{
    Expression, ExpressionKind, SlotDeclaration, SlotMutability, Statement, StatementKind,
};
use ns_syntax::Span;

use super::edits::{self, Edit};
use super::names;
use super::walk::{Binding, Scope, Section, SectionKind};
use super::{Hint, HintKind};

pub(super) fn hints(section: &Section<'_>, scope: &Scope<'_>, text: &str, hints: &mut Vec<Hint>) {
    if section.slots.iter().any(|slot| slot.simultaneous) {
        return;
    }
    let last_initialized = section
        .slots
        .iter()
        .rposition(|slot| slot.initializer.is_some());
    for (index, statement) in section.statements.iter().enumerate() {
        let Some((target, value)) = assignment(statement) else {
            continue;
        };
        let Some(position) = bare_slot(section.slots, target) else {
            continue;
        };
        // A closure answers its last statement's value; an initializer has
        // none.
        if section.kind == SectionKind::Closure && index + 1 == section.statements.len() {
            continue;
        }
        // A comment attached to the statement would describe the next one.
        if !statement.comments.is_empty() || names::expression_mentions(value, target) {
            continue;
        }
        // After other statements only when none of them mentions the local:
        // an initializer's closure may read it too.
        let movable = index == 0
            || (settled(value, scope)
                && !section.statements[..index]
                    .iter()
                    .any(|earlier| names::mentions(earlier, target))
                && !section.slots.iter().any(|slot| {
                    slot.initializer
                        .as_ref()
                        .is_some_and(|initializer| names::expression_mentions(initializer, target))
                }));
        if !movable {
            continue;
        }
        let Some(deletion) = edits::delete_statement(text, statement) else {
            continue;
        };
        let placed = match last_initialized {
            Some(last) if position < last => {
                reordered(text, section.slots, position, last, statement, value)
            }
            _ => Some(in_place(text, &section.slots[position], statement, value)),
        };
        let Some(mut fix) = placed else {
            continue;
        };
        fix.push(deletion);
        hints.push(Hint {
            kind: HintKind::InitializeInDeclaration,
            name: target.to_string(),
            span: section.slots[position].name.span,
            section: section.span,
            edits: fix,
        });
    }
}

/// The target and value of a statement that is one assignment `x:: e`, not
/// parenthesized and not a chain.
fn assignment(statement: &Statement) -> Option<(&str, &Expression)> {
    let StatementKind::Expression(expression) = &statement.kind else {
        return None;
    };
    let ExpressionKind::Assign { target, value, .. } = &expression.kind else {
        return None;
    };
    if !expression.parentheses.is_empty() || matches!(value.kind, ExpressionKind::Assign { .. }) {
        return None;
    }
    Some((target.name.as_str(), value))
}

/// The position of the bare slot `name` in `slots`.
fn bare_slot(slots: &[SlotDeclaration], name: &str) -> Option<usize> {
    slots.iter().position(|slot| {
        slot.name.name == name
            && slot.initializer.is_none()
            && slot.mutability == SlotMutability::MutableBare
    })
}

/// Whether `value` can be evaluated earlier than where it stands without
/// anyone seeing the difference: a literal, `self`, or a name bound to a
/// parameter or a read-only local, none of which can fail or change. A
/// read-only local of the same section is initialized before the moved
/// slot, since the slot then stands after the last initialized one.
fn settled(value: &Expression, scope: &Scope<'_>) -> bool {
    match &value.kind {
        ExpressionKind::Literal(_) => true,
        ExpressionKind::Identifier(identifier) => {
            identifier.name == "self"
                || matches!(
                    scope.binding(&identifier.name),
                    Some(Binding::Parameter | Binding::ReadOnlyLocal)
                )
        }
        _ => false,
    }
}

fn in_place(
    text: &str,
    slot: &SlotDeclaration,
    statement: &Statement,
    value: &Expression,
) -> Vec<Edit> {
    let initializer = edits::initializer(text, value, statement.span.start, slot.span.start);
    // A slot written with its optional `.` keeps it, the initializer going
    // before it.
    let (end, initializer) = match edits::bare_period(text, slot) {
        Some(period) => (
            period.start,
            initializer
                .strip_suffix('.')
                .unwrap_or_default()
                .to_string(),
        ),
        None => (edits::declaration_end(slot), initializer),
    };
    vec![Edit {
        span: Span::new(end, end),
        text: initializer,
    }]
}

/// The slot at `position` moved, with its initializer, after the slot at
/// `last`, the last initialized one; `None` when a comment stands in what
/// the move deletes or copies, or after the declaration it moves behind:
/// a comment stays beside the code it was written beside.
fn reordered(
    text: &str,
    slots: &[SlotDeclaration],
    position: usize,
    last: usize,
    statement: &Statement,
    value: &Expression,
) -> Option<Vec<Edit>> {
    let slot = &slots[position];
    let following = &slots[position + 1];
    // The slot's own `.` goes with the removal; the moved declaration gets
    // the initializer's.
    let separator = &text[edits::written_end(text, slot) as usize..following.span.start as usize];
    let removal = if position == 0 {
        Span::new(slot.span.start, following.span.start)
    } else {
        Span::new(
            edits::written_end(text, &slots[position - 1]),
            edits::written_end(text, slot),
        )
    };
    let destination = slots[last].span.end;
    if edits::holds_a_comment(&text[removal.start as usize..removal.end as usize])
        || edits::holds_a_comment(separator)
        || edits::holds_a_comment(edits::rest_of_line(text, destination))
    {
        return None;
    }
    let declaration = format!(
        "{separator}{}{}",
        &text[slot.span.start as usize..edits::declaration_end(slot) as usize],
        edits::initializer(text, value, statement.span.start, slots[last].span.start)
    );
    Some(vec![
        Edit {
            span: removal,
            text: String::new(),
        },
        Edit {
            span: Span::new(destination, destination),
            text: declaration,
        },
    ])
}
