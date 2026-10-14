//! Hint B: a slot declared with `::=` that nothing assigns again can be
//! declared with `=`. Only `name::` writes a local -- an implicit-receiver
//! send `name: value` is resolved through the classes, never the locals
//! (`ns-runtime`'s `resolver/mod.rs`) -- so a local is read-only when no
//! `name::` stands in its body.
//!
//! A class's header slots get no hint B. A public or protected slot's setter
//! may be sent from anywhere; a private slot's fix could lift a value
//! object's refusal of a mutable slot (`MutableValueSlot`), and whether a
//! class's instances are values is not in its declaration: `Value` can be
//! mixed in above it later.

use crate::ast::SlotMutability;
use crate::span::Span;

use super::edits::{self, Edit};
use super::names;
use super::walk::Section;
use super::{Hint, HintKind};

/// Hint B for the locals of a body's section.
pub(super) fn local_hints(section: &Section<'_>, text: &str, hints: &mut Vec<Hint>) {
    if section.slots.iter().any(|slot| slot.simultaneous) {
        return;
    }
    let assigned = names::assigned_in(section.slots, section.statements);
    for slot in section.slots {
        if slot.mutability != SlotMutability::MutableInitialized
            || assigned.contains(slot.name.name.as_str())
        {
            continue;
        }
        push(slot, section.span, text, hints);
    }
}

fn push(slot: &crate::ast::SlotDeclaration, section: Span, text: &str, hints: &mut Vec<Hint>) {
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
