//! Static member-set and superclass-name computation for classes, used only
//! to pick a lexical-send depth `k` (`docs/design/dispatch.md`,
//! "Resolution", step 2) -- never to resolve which concrete method body
//! runs (that is `dispatch.rs`'s job, at runtime, over the real object
//! graph).

use std::collections::HashSet;

use ns_syntax::ast;

use crate::Symbol;

/// Names Object supports with no Newspeak-source declaration to find --
/// resolving a superclass-clause receiver (or a bare implicit send) against
/// these is the base case that lets a lexical/inheritance search bottom out.
/// Extend this list as the core library gains classes and public selectors
/// that user code may reasonably reference by bare name from a superclass
/// clause (in practice, just the class *names* below matter for superclass
/// resolution -- Object's own instance-side selectors matter for implicit
/// sends that fall all the way through to Object).
pub(super) const BUILTIN_CLASS_NAMES: &[&str] = &[
    "Object",
    "Boolean",
    "True",
    "False",
    "Integer",
    "Float",
    "String",
    "Symbol",
    "Character",
    "Array",
    "OrderedCollection",
    "Nil",
    "Block",
];

pub(super) fn is_builtin_class_name(name: Symbol) -> bool {
    BUILTIN_CLASS_NAMES.contains(&name.as_str())
}

/// The members one side of a class declaration *declares*, each under its
/// full selector, in declaration order and with any repeats kept -- what
/// `newspeak-spec.pdf` §5.7 searches, "a method, slot or class named s":
///
/// - every method, under its whole selector (`at:put:` declares `at:put:`,
///   never `at`);
/// - every slot's getter, under the slot's name, and a mutable slot's setter
///   under the name with `:` appended -- §6.3.2 has both "implicitly defined
///   by the slot declaration", and an immutable slot has "no setter method";
/// - every nested class, whose accessor has "the same name" (§6).
///
/// Nothing inherited: §5.7 asks where a member is *declared*.
fn declared_members(body: &ast::ClassBody) -> Vec<Symbol> {
    let mut members = Vec::new();
    for slot in &body.slots {
        members.push(Symbol::intern(&slot.name.name));
        if slot.mutability != ast::SlotMutability::Immutable {
            members.push(Symbol::intern(&format!("{}:", slot.name.name)));
        }
    }
    for method in &body.methods {
        members.push(Symbol::intern(
            &crate::interpreter::instantiate::method_name(&method.pattern),
        ));
    }
    for nested in &body.nested_classes {
        members.push(Symbol::intern(&nested.name.name));
    }
    members
}

/// The members one class declaration declares, both sides together
/// (`docs/design/dispatch.md`, "Resolution": the class side's own scope is an
/// open question the specification does not settle).
pub(super) fn own_member_names(declaration: &ast::ClassDeclaration) -> HashSet<Symbol> {
    let mut names: HashSet<Symbol> = declared_members(&declaration.instance_side)
        .into_iter()
        .collect();
    if let Some(class_side) = &declaration.class_side {
        names.extend(declared_members(class_side));
    }
    names
}

/// The declaration's factory selector: the selector its own message pattern
/// spells out, or `new` when it writes none (`newspeak-spec.pdf` §6.1's "a
/// class definition need not include ... a message pattern", defaulted the
/// same way `interpreter::instantiate::build_mixin` defaults the mixin's
/// `Factory`). One implementation, so the resolver's notion of "the factory
/// selector" cannot drift from the one the metaclass mixin is actually built
/// with.
fn factory_selector_of(declaration: &ast::ClassDeclaration) -> Symbol {
    match &declaration.factory {
        Some(pattern) => Symbol::intern(&crate::interpreter::instantiate::method_name(pattern)),
        None => Symbol::NEW,
    }
}

/// The names one side of `declaration` declares more than once, each once.
/// The specification states no rule for two ordinary members of one name;
/// the canonical compiler refuses them
/// (`prior-art/newspeak/Newspeak2SqueakCompilation.ns`,
/// `checkForDuplicateNames:`), and a mixin's method table cannot hold two
/// members under one selector. Each side is its own table, so a name the
/// instance side and the class side both declare is not a duplicate.
///
/// The class side's table is not only what it declares, though: §6.3 makes
/// the primary factory a member of the metaclass mixin the class side
/// becomes, so a class-side method under the factory's own selector is a
/// second entry for that one name -- refused here the same way, by seeding
/// the class side's `seen` set with it before walking its declared members.
/// The image side (`image::mixins::check_member_names`) checks the same
/// collision on a decoded class-side row, against the mixin's own encoded
/// `Factory` -- populated, when the mixin was built, by this same
/// `method_name` computation -- so a source-level refusal here and an
/// image-level refusal there agree on exactly the same programs. Primordial
/// Soup refuses it too (`prior-art/primordialsoup/newspeak/Intermediates.ns`,
/// `checkNameConflictsForFactory:`); SOMns lets the declared method win and
/// files the factory under an unreachable name, after which the class no
/// longer instantiates and nothing says so.
pub(super) fn duplicate_member_names(declaration: &ast::ClassDeclaration) -> Vec<Symbol> {
    let mut duplicates = Vec::new();
    let mut seen = HashSet::new();
    for member in declared_members(&declaration.instance_side) {
        if !seen.insert(member) && !duplicates.contains(&member) {
            duplicates.push(member);
        }
    }
    if let Some(class_side) = declaration.class_side.as_ref() {
        let mut seen = HashSet::new();
        seen.insert(factory_selector_of(declaration));
        for member in declared_members(class_side) {
            if !seen.insert(member) && !duplicates.contains(&member) {
                duplicates.push(member);
            }
        }
    }
    duplicates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_names_are_recognized() {
        assert!(is_builtin_class_name(Symbol::intern("Object")));
        assert!(!is_builtin_class_name(Symbol::intern("NotABuiltin")));
    }
}
