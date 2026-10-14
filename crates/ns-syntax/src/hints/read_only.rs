//! Hint B: a slot declared with `::=` that nothing assigns again can be
//! declared with `=`. Only `name::` writes a local -- an implicit-receiver
//! send `name: value` is resolved through the classes, never the locals
//! (`ns-runtime`'s `resolver/mod.rs`) -- so a local is read-only when no
//! `name::` stands in its body.

use crate::ast::{AccessModifier, ClassDeclaration, SlotMutability};
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

/// Hint B for a class's private header slots. A private member is reached
/// only by sends from within the class that declares it (§5.8, §5.9,
/// §6.3.1); `Message>>sendTo:` is an ordinary public send (§5.5), and no
/// program has a reflective slot write. So a private slot nothing in its
/// class declaration writes -- by `name::` or by a send of `name:` -- is
/// never written. A public or protected slot's setter may be sent from
/// anywhere, so it gets no hint.
pub(super) fn header_hints(declaration: &ClassDeclaration, text: &str, hints: &mut Vec<Hint>) {
    let header = &declaration.instance_side;
    if header.header_slots.iter().any(|slot| slot.simultaneous) {
        return;
    }
    let mut written = None;
    for slot in &header.header_slots {
        if slot.access != Some(AccessModifier::Private)
            || slot.mutability != SlotMutability::MutableInitialized
        {
            continue;
        }
        let written = written.get_or_insert_with(|| names::written_in_class(declaration));
        if !written.contains(slot.name.name.as_str()) {
            push(slot, header.span, text, hints);
        }
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
