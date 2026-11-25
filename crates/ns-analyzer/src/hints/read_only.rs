//! Hint B: a local declared with `::=` that nothing assigns again can be
//! declared with `=`. `name:: value` and the implicit-receiver send
//! `name: value` write a local (`ns-runtime`'s `resolver/local_writes.rs`),
//! so a local is read-only when neither stands in its body.
//!
//! Nor may a closure see the local before its initializer has run: the
//! optimized lowering copies a read-only local into a closure when the
//! closure is made (`docs/design/compiler.md`), so a closure made by its
//! own or an earlier slot's initializer would read nil where the `::=`
//! local, shared with the closure, gives the later value.

use ns_syntax::Span;
use ns_syntax::ast::{SlotDeclaration, SlotMutability};

use super::edits::{self, Edit};
use super::names;
use super::walk::Section;
use super::{Hint, HintKind};

/// Hint B for the locals of a section.
pub(super) fn hints(section: &Section<'_>, text: &str, hints: &mut Vec<Hint>) {
    if section.slots.iter().any(|slot| slot.simultaneous) {
        return;
    }
    let assigned = names::assigned_in(section.slots, section.statements);
    for (position, slot) in section.slots.iter().enumerate() {
        if slot.mutability != SlotMutability::MutableInitialized
            || assigned.contains(slot.name.name.as_str())
            || captured_early(&section.slots[..=position], &slot.name.name)
        {
            continue;
        }
        push(slot, section.span, text, hints);
    }
}

/// Whether an initializer of `slots` -- the local's own and those before
/// it -- mentions `name`. Any mention counts, a closure's or not: a closure
/// is what can read the local early, and telling one apart costs more than
/// the hint is worth.
fn captured_early(slots: &[SlotDeclaration], name: &str) -> bool {
    slots.iter().any(|slot| {
        slot.initializer
            .as_ref()
            .is_some_and(|initializer| names::expression_mentions(initializer, name))
    })
}

fn push(slot: &SlotDeclaration, section: Span, text: &str, hints: &mut Vec<Hint>) {
    let Some(operator) = edits::assignment_operator(text, slot) else {
        return;
    };
    hints.push(Hint {
        kind: HintKind::ReadOnly,
        name: slot.name.name.clone(),
        span: slot.name.span,
        section,
        edits: vec![Edit {
            span: operator,
            text: "=".to_string(),
        }],
    });
}
